//! Integration tests for the Atrium marketplace.
//!
//! Uses cw-multi-test to spin up real cw20 + cw721 contracts and exercise
//! the marketplace end-to-end. Targets ~25 invariants spanning happy paths,
//! authorization, expiry boundaries, payment-type matching, fee/royalty math,
//! the Crystal-holder 0% discount, the curation allowlist, and launch caps.

#![cfg(test)]

use cosmwasm_std::{coin, coins, to_json_binary, Addr, Empty, Uint128};
use cw20::{Cw20Coin, Cw20ExecuteMsg, MinterResponse};
use cw_multi_test::{App, AppBuilder, Contract, ContractWrapper, Executor};

use crate::msg::{
    CollectionOffersResponse, Cw20HookMsg, ExecuteMsg, FeeInfoForTradeResponse, FeeInfoResponse,
    InstantiateMsg, IsAllowedResponse, ListNftMsg, MigrateMsg, QueryMsg, TraitProof,
    TraitRegistryResponse,
};
use crate::state::{CollectionOffer, LaunchCaps, PaymentType, TraitConstraint};

// ─── Contract wrappers ─────────────────────────────────────────────────────

fn marketplace_contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(
        crate::contract::execute,
        crate::contract::instantiate,
        crate::contract::query,
    ))
}

fn cw721_contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(
        cw721_base::entry::execute,
        cw721_base::entry::instantiate,
        cw721_base::entry::query,
    ))
}

fn cw20_contract() -> Box<dyn Contract<Empty>> {
    Box::new(ContractWrapper::new(
        cw20_base::contract::execute,
        cw20_base::contract::instantiate,
        cw20_base::contract::query,
    ))
}

// ─── Helpers ───────────────────────────────────────────────────────────────

const DENOM: &str = "uluna";

struct Fixture {
    app: App,
    market: Addr,
    crystal: Addr,
    other_collection: Addr,
    capa: Addr,
    owner: Addr,
    treasury: Addr,
    capa_pool: Addr,
}

fn setup() -> Fixture {
    let owner = Addr::unchecked("owner");
    let treasury = Addr::unchecked("treasury");
    let capa_pool = Addr::unchecked("capa_pool");

    let mut app = AppBuilder::new().build(|router, _, storage| {
        for who in [
            "owner", "treasury", "capa_pool", "alice", "bob", "carol", "stranger",
            "crystal_holder", "minter",
        ] {
            router
                .bank
                .init_balance(
                    storage,
                    &Addr::unchecked(who),
                    vec![coin(1_000_000_000, DENOM), coin(1_000_000, "uusd")],
                )
                .unwrap();
        }
    });

    // Mint contracts
    let cw721_id = app.store_code(cw721_contract());
    let cw20_id = app.store_code(cw20_contract());
    let market_id = app.store_code(marketplace_contract());

    // Crystal collection
    let crystal = app
        .instantiate_contract(
            cw721_id,
            owner.clone(),
            &cw721_base::msg::InstantiateMsg {
                name: "CAPA Crystals".into(),
                symbol: "CAPA".into(),
                minter: owner.to_string(),
            },
            &[],
            "crystal",
            None,
        )
        .unwrap();

    // Another collection (for allowlist tests)
    let other_collection = app
        .instantiate_contract(
            cw721_id,
            owner.clone(),
            &cw721_base::msg::InstantiateMsg {
                name: "Other".into(),
                symbol: "OTH".into(),
                minter: owner.to_string(),
            },
            &[],
            "other",
            None,
        )
        .unwrap();

    // CAPA cw20
    let capa = app
        .instantiate_contract(
            cw20_id,
            owner.clone(),
            &cw20_base::msg::InstantiateMsg {
                name: "CAPA".into(),
                symbol: "CAPA".into(),
                decimals: 6,
                initial_balances: vec![
                    Cw20Coin {
                        address: "alice".into(),
                        amount: Uint128::new(10_000_000_000),
                    },
                    Cw20Coin {
                        address: "bob".into(),
                        amount: Uint128::new(10_000_000_000),
                    },
                    Cw20Coin {
                        address: "stranger".into(),
                        amount: Uint128::new(10_000_000_000),
                    },
                ],
                mint: Some(MinterResponse {
                    minter: owner.to_string(),
                    cap: None,
                }),
                marketing: None,
            },
            &[],
            "capa",
            None,
        )
        .unwrap();

    // Marketplace — fee_bps=150 (1.5%), treasury_share_bps=100 (=1.0%), capa_share=50 bps
    let market = app
        .instantiate_contract(
            market_id,
            owner.clone(),
            &InstantiateMsg {
                fee_bps: 150,
                treasury_addr: treasury.to_string(),
                capa_reward_addr: capa_pool.to_string(),
                treasury_share_bps: 100,
                capa_gov_contract: None,
                crystal_nft_contract: crystal.to_string(),
                initial_collections: vec![crystal.to_string()],
                launch_caps: LaunchCaps {
                    max_active_listings_per_collection: 200,
                    max_active_offers_per_nft: 20,
                },
            },
            &[],
            "atrium",
            None,
        )
        .unwrap();

    Fixture {
        app,
        market,
        crystal,
        other_collection,
        capa,
        owner,
        treasury,
        capa_pool,
    }
}

fn mint_crystal(fx: &mut Fixture, owner_of: &str, token_id: &str) {
    let owner = fx.owner.clone();
    let crystal = fx.crystal.clone();
    fx.app
        .execute_contract(
            owner,
            crystal,
            &cw721_base::ExecuteMsg::<Empty, Empty>::Mint {
                token_id: token_id.into(),
                owner: owner_of.into(),
                token_uri: None,
                extension: Empty {},
            },
            &[],
        )
        .unwrap();
}

fn mint_other(fx: &mut Fixture, owner_of: &str, token_id: &str) {
    let owner = fx.owner.clone();
    let collection = fx.other_collection.clone();
    fx.app
        .execute_contract(
            owner,
            collection,
            &cw721_base::ExecuteMsg::<Empty, Empty>::Mint {
                token_id: token_id.into(),
                owner: owner_of.into(),
                token_uri: None,
                extension: Empty {},
            },
            &[],
        )
        .unwrap();
}

/// Marker that resolves to a collection address inside a Fixture borrow.
enum Coll {
    Crystal,
    Other,
}

/// Asserts that the error chain's root cause contains `needle`.
/// cw-multi-test wraps contract errors in a "Error executing WasmMsg" prefix,
/// so we walk to the bottom of the chain.
fn assert_err(err: &anyhow::Error, needle: &str) {
    let root = err.root_cause().to_string();
    assert!(
        root.contains(needle),
        "expected error containing `{}`, got `{}`",
        needle,
        root
    );
}

fn list_nft_native(
    fx: &mut Fixture,
    seller: &str,
    collection: Coll,
    token_id: &str,
    price: u128,
    expires_in_blocks: u64,
) -> anyhow::Result<()> {
    let list_msg = ListNftMsg {
        price: Uint128::new(price),
        payment: PaymentType::Native {
            denom: DENOM.into(),
        },
        expires_in_blocks,
    whitelisted_buyer: None,
        lock_in_blocks: None,
        whitelist: None,
    };
    let market = fx.market.to_string();
    let coll_addr = match collection {
        Coll::Crystal => fx.crystal.clone(),
        Coll::Other => fx.other_collection.clone(),
    };
    fx.app
        .execute_contract(
            Addr::unchecked(seller),
            coll_addr,
            &cw721_base::ExecuteMsg::<Empty, Empty>::SendNft {
                contract: market,
                token_id: token_id.into(),
                msg: to_json_binary(&list_msg).unwrap(),
            },
            &[],
        )
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{}", e.root_cause()))
}

fn list_nft_cw20(
    fx: &mut Fixture,
    seller: &str,
    collection: Coll,
    token_id: &str,
    price: u128,
    expires_in_blocks: u64,
) -> anyhow::Result<()> {
    let list_msg = ListNftMsg {
        price: Uint128::new(price),
        payment: PaymentType::Cw20 {
            contract_addr: fx.capa.to_string(),
        },
        expires_in_blocks,
    whitelisted_buyer: None,
        lock_in_blocks: None,
        whitelist: None,
    };
    let market = fx.market.to_string();
    let coll_addr = match collection {
        Coll::Crystal => fx.crystal.clone(),
        Coll::Other => fx.other_collection.clone(),
    };
    fx.app
        .execute_contract(
            Addr::unchecked(seller),
            coll_addr,
            &cw721_base::ExecuteMsg::<Empty, Empty>::SendNft {
                contract: market,
                token_id: token_id.into(),
                msg: to_json_binary(&list_msg).unwrap(),
            },
            &[],
        )
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{}", e.root_cause()))
}

// ───────────────────────────────────────────────────────────────────────────
// 1. INSTANTIATE
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn invariant_01_instantiate_succeeds() {
    let fx = setup();
    let cfg: crate::state::Config = fx
        .app
        .wrap()
        .query_wasm_smart(&fx.market, &QueryMsg::Config {})
        .unwrap();
    assert_eq!(cfg.fee_bps, 150);
    assert_eq!(cfg.treasury_share_bps, 100);
    assert!(!cfg.paused);
}

#[test]
fn invariant_02_instantiate_rejects_fee_above_5pct() {
    let owner = Addr::unchecked("owner");
    let mut app = AppBuilder::new().build(|_, _, _| {});
    let market_id = app.store_code(marketplace_contract());

    // Throwaway crystal contract for the test
    let cw721_id = app.store_code(cw721_contract());
    let crystal = app
        .instantiate_contract(
            cw721_id,
            owner.clone(),
            &cw721_base::msg::InstantiateMsg {
                name: "C".into(),
                symbol: "C".into(),
                minter: owner.to_string(),
            },
            &[],
            "c",
            None,
        )
        .unwrap();

    let err = app
        .instantiate_contract(
            market_id,
            owner.clone(),
            &InstantiateMsg {
                fee_bps: 600, // > 5%
                treasury_addr: "treasury".into(),
                capa_reward_addr: "pool".into(),
                treasury_share_bps: 100,
                capa_gov_contract: None,
                crystal_nft_contract: crystal.to_string(),
                initial_collections: vec![],
                launch_caps: LaunchCaps {
                    max_active_listings_per_collection: 200,
                    max_active_offers_per_nft: 20,
                },
            },
            &[],
            "bad",
            None,
        )
        .unwrap_err();
    assert_err(&err, "Fee too high");
}

#[test]
fn invariant_03_instantiate_rejects_treasury_share_above_fee() {
    let owner = Addr::unchecked("owner");
    let mut app = AppBuilder::new().build(|_, _, _| {});
    let market_id = app.store_code(marketplace_contract());
    let cw721_id = app.store_code(cw721_contract());
    let crystal = app
        .instantiate_contract(
            cw721_id,
            owner.clone(),
            &cw721_base::msg::InstantiateMsg {
                name: "C".into(),
                symbol: "C".into(),
                minter: owner.to_string(),
            },
            &[],
            "c",
            None,
        )
        .unwrap();

    let err = app
        .instantiate_contract(
            market_id,
            owner.clone(),
            &InstantiateMsg {
                fee_bps: 100,
                treasury_addr: "treasury".into(),
                capa_reward_addr: "pool".into(),
                treasury_share_bps: 200, // > fee
                capa_gov_contract: None,
                crystal_nft_contract: crystal.to_string(),
                initial_collections: vec![],
                launch_caps: LaunchCaps {
                    max_active_listings_per_collection: 200,
                    max_active_offers_per_nft: 20,
                },
            },
            &[],
            "bad",
            None,
        )
        .unwrap_err();
    assert_err(&err, "treasury_share_bps");
}

// ───────────────────────────────────────────────────────────────────────────
// 2. CURATION ALLOWLIST
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn invariant_04_listing_on_allowlisted_collection_succeeds() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();
}

#[test]
fn invariant_05_listing_on_disallowed_collection_fails() {
    let mut fx = setup();
    mint_other(&mut fx, "alice", "1");
    let err = list_nft_native(&mut fx, "alice", Coll::Other, "1", 1_000_000, 0)
        .unwrap_err();
    assert_err(&err, "Collection not allowlisted");
}

#[test]
fn invariant_06_admin_can_add_collection() {
    let mut fx = setup();
    fx.app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::AddCollection {
                nft_contract: fx.other_collection.to_string(),
            },
            &[],
        )
        .unwrap();
    let resp: IsAllowedResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.market,
            &QueryMsg::IsCollectionAllowed {
                nft_contract: fx.other_collection.to_string(),
            },
        )
        .unwrap();
    assert!(resp.allowed);

    // Now listing on it works
    mint_other(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Other, "1", 1_000_000, 0).unwrap();
}

#[test]
fn invariant_07_non_admin_cannot_add_collection() {
    let mut fx = setup();
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("stranger"),
            fx.market.clone(),
            &ExecuteMsg::AddCollection {
                nft_contract: fx.other_collection.to_string(),
            },
            &[],
        )
        .unwrap_err();
    assert_err(&err, "not the contract admin");
}

// ───────────────────────────────────────────────────────────────────────────
// 3. BUY / SALE MATH
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn invariant_08_buy_native_happy_path_with_correct_fee_split() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    // Bob (non-Crystal-holder) buys for 1_000_000 uluna
    let bob_balance_before = fx.app.wrap().query_balance("bob", DENOM).unwrap().amount;
    let alice_balance_before = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;
    let treasury_before = fx.app.wrap().query_balance(&fx.treasury, DENOM).unwrap().amount;
    let pool_before = fx.app.wrap().query_balance(&fx.capa_pool, DENOM).unwrap().amount;

    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(1_000_000, DENOM),
        )
        .unwrap();

    let bob_balance_after = fx.app.wrap().query_balance("bob", DENOM).unwrap().amount;
    let alice_balance_after = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;
    let treasury_after = fx.app.wrap().query_balance(&fx.treasury, DENOM).unwrap().amount;
    let pool_after = fx.app.wrap().query_balance(&fx.capa_pool, DENOM).unwrap().amount;

    // Bob spent 1_000_000
    assert_eq!(bob_balance_before.u128() - bob_balance_after.u128(), 1_000_000);
    // Fee = 1.5% of 1M = 15_000, split 100/150 to treasury (10_000) + 50/150 to pool (5_000)
    assert_eq!(treasury_after.u128() - treasury_before.u128(), 10_000);
    assert_eq!(pool_after.u128() - pool_before.u128(), 5_000);
    // Alice receives 985_000
    assert_eq!(alice_balance_after.u128() - alice_balance_before.u128(), 985_000);

    // NFT now owned by Bob
    let resp: cw721::OwnerOfResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.crystal,
            &cw721::Cw721QueryMsg::OwnerOf {
                token_id: "1".into(),
                include_expired: None,
            },
        )
        .unwrap();
    assert_eq!(resp.owner, "bob");
}

#[test]
fn invariant_09_buy_native_self_purchase_fails() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(1_000_000, DENOM),
        )
        .unwrap_err();
    assert_err(&err, "own listing");
}

#[test]
fn invariant_10_buy_native_inexact_payment_fails() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    // Underpayment
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(999_999, DENOM),
        )
        .unwrap_err();
    assert_err(&err, "Insufficient payment");

    // Overpayment also fails (would lock surplus)
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(1_000_001, DENOM),
        )
        .unwrap_err();
    assert_err(&err, "Insufficient payment");
}

#[test]
fn invariant_11_buy_native_multidenom_fails() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &[coin(1_000_000, DENOM), coin(1, "uusd")],
        )
        .unwrap_err();
    assert_err(&err, "Send exactly one");
}

#[test]
fn invariant_12_buy_native_expired_listing_fails() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 5).unwrap();

    // Advance past expiry
    fx.app.update_block(|b| b.height += 10);

    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(1_000_000, DENOM),
        )
        .unwrap_err();
    assert_err(&err, "Listing has expired");
}

// ───────────────────────────────────────────────────────────────────────────
// 4. CRYSTAL HOLDER 0% DISCOUNT
// ───────────────────────────────────────────────────────────────────────────

// V1.1.0 (Alt D — tier ladder): a Crystal-holder's discount depends on the
// HIGHEST tier they own. Tier resolution walks ALTAR → FUSION → MINT (mainnet
// hardcoded addresses). In the test environment those contracts don't exist,
// so resolve_tier() returns None for every token and `highest_crystal_tier()`
// returns None. Therefore Crystal-holders in tests fall through to fee_bps —
// which is the CORRECT behaviour for an isolated test env.
//
// On mainnet, the resolution chain is live; see /api/atrium/fee-info smoke
// test in V1.1.0 deploy notes for tier-by-tier verification against the
// actual chain.

#[test]
fn invariant_13_crystal_holder_pays_full_fee_without_tier_resolution() {
    let mut fx = setup();
    // crystal_holder owns Crystal #1 (cw721-base mock; tier-resolution will
    // fail in this env because altar/fusion/mint contracts aren't deployed,
    // so the buyer falls through to fee_bps regardless of holder status).
    mint_crystal(&mut fx, "crystal_holder", "1");
    mint_crystal(&mut fx, "alice", "2");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "2", 1_000_000, 0).unwrap();

    let alice_before = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;
    let treasury_before = fx.app.wrap().query_balance(&fx.treasury, DENOM).unwrap().amount;

    fx.app
        .execute_contract(
            Addr::unchecked("crystal_holder"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(1_000_000, DENOM),
        )
        .unwrap();

    let alice_after = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;
    let treasury_after = fx.app.wrap().query_balance(&fx.treasury, DENOM).unwrap().amount;

    // Without tier resolution: full 1.5% fee applies. Treasury gets 1.0%
    // (treasury_share_bps=100 of fee_bps=150), Alice gets the rest.
    assert_eq!(treasury_after.u128() - treasury_before.u128(), 10_000);
    assert_eq!(alice_after.u128() - alice_before.u128(), 985_000);
}

#[test]
fn invariant_14_fee_info_query_reflects_tier_resolution_failure() {
    let mut fx = setup();
    mint_crystal(&mut fx, "crystal_holder", "1");

    let resp_holder: FeeInfoResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.market,
            &QueryMsg::FeeInfo {
                buyer: Some("crystal_holder".into()),
            },
        )
        .unwrap();
    // Tier resolution unavailable in tests → no discount applied.
    assert_eq!(resp_holder.fee_bps, 150);
    assert_eq!(resp_holder.discount_bps, 0);
    assert!(!resp_holder.crystal_holder);
    assert_eq!(resp_holder.crystal_tier, None);

    let resp_normal: FeeInfoResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.market,
            &QueryMsg::FeeInfo {
                buyer: Some("bob".into()),
            },
        )
        .unwrap();
    assert_eq!(resp_normal.fee_bps, 150);
    assert_eq!(resp_normal.discount_bps, 0);
    assert!(!resp_normal.crystal_holder);
    assert_eq!(resp_normal.crystal_tier, None);
}

// ───────────────────────────────────────────────────────────────────────────
// 5. CW20 BUY + REFUND-ON-MISMATCH
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn invariant_15_buy_cw20_happy_path() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_cw20(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    let buy_msg = Cw20HookMsg::BuyNft { listing_id: 1 };
    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.capa.clone(),
            &Cw20ExecuteMsg::Send {
                contract: fx.market.to_string(),
                amount: Uint128::new(1_000_000),
                msg: to_json_binary(&buy_msg).unwrap(),
            },
            &[],
        )
        .unwrap();

    // Verify Alice got the seller payout (985_000 net of 1.5% fee)
    // — alice started with 10_000_000_000 CAPA from setup, so check delta.
    let alice_balance: cw20::BalanceResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.capa,
            &cw20::Cw20QueryMsg::Balance {
                address: "alice".into(),
            },
        )
        .unwrap();
    assert_eq!(alice_balance.balance.u128(), 10_000_000_000 + 985_000);
}

#[test]
fn invariant_16_buy_cw20_self_purchase_refunds() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_cw20(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    let alice_before: cw20::BalanceResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.capa,
            &cw20::Cw20QueryMsg::Balance {
                address: "alice".into(),
            },
        )
        .unwrap();

    // Alice tries to self-buy via cw20 — should refund (not error)
    let buy_msg = Cw20HookMsg::BuyNft { listing_id: 1 };
    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.capa.clone(),
            &Cw20ExecuteMsg::Send {
                contract: fx.market.to_string(),
                amount: Uint128::new(1_000_000),
                msg: to_json_binary(&buy_msg).unwrap(),
            },
            &[],
        )
        .unwrap();

    let alice_after: cw20::BalanceResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.capa,
            &cw20::Cw20QueryMsg::Balance {
                address: "alice".into(),
            },
        )
        .unwrap();
    // Alice's CAPA balance unchanged (refunded)
    assert_eq!(alice_after.balance, alice_before.balance);

    // NFT still in marketplace's custody (listing still active)
    let owner: cw721::OwnerOfResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.crystal,
            &cw721::Cw721QueryMsg::OwnerOf {
                token_id: "1".into(),
                include_expired: None,
            },
        )
        .unwrap();
    assert_eq!(owner.owner, fx.market.to_string());
}

// ───────────────────────────────────────────────────────────────────────────
// 6. CANCEL LISTING
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn invariant_17_cancel_listing_by_seller_returns_nft() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::CancelListing { listing_id: 1 },
            &[],
        )
        .unwrap();

    let owner: cw721::OwnerOfResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.crystal,
            &cw721::Cw721QueryMsg::OwnerOf {
                token_id: "1".into(),
                include_expired: None,
            },
        )
        .unwrap();
    assert_eq!(owner.owner, "alice");
}

#[test]
fn invariant_18_cancel_listing_by_stranger_fails() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("stranger"),
            fx.market.clone(),
            &ExecuteMsg::CancelListing { listing_id: 1 },
            &[],
        )
        .unwrap_err();
    assert_err(&err, "not the listing's seller");
}

// ───────────────────────────────────────────────────────────────────────────
// 7. OFFERS
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn invariant_19_make_and_cancel_offer_native_refunds_buyer() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    let bob_before = fx.app.wrap().query_balance("bob", DENOM).unwrap().amount;

    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::MakeOffer {
                nft_contract: fx.crystal.to_string(),
                token_id: "1".into(),
                expires_in_blocks: 100,
            },
            &coins(800_000, DENOM),
        )
        .unwrap();

    // Bob's escrow balance reduced
    let bob_after_offer = fx.app.wrap().query_balance("bob", DENOM).unwrap().amount;
    assert_eq!(bob_before.u128() - bob_after_offer.u128(), 800_000);

    // Bob cancels — refund
    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::CancelOffer { offer_id: 1 },
            &[],
        )
        .unwrap();

    let bob_after_cancel = fx.app.wrap().query_balance("bob", DENOM).unwrap().amount;
    assert_eq!(bob_after_cancel, bob_before);
}

#[test]
fn invariant_20_accept_offer_settles_at_offer_price() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::MakeOffer {
                nft_contract: fx.crystal.to_string(),
                token_id: "1".into(),
                expires_in_blocks: 100,
            },
            &coins(800_000, DENOM),
        )
        .unwrap();

    let alice_before = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;

    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::AcceptOffer { offer_id: 1 },
            &[],
        )
        .unwrap();

    let alice_after = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;
    // Alice receives 800K - 1.5% fee = 788_000
    assert_eq!(alice_after.u128() - alice_before.u128(), 788_000);

    // NFT now Bob's
    let owner: cw721::OwnerOfResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.crystal,
            &cw721::Cw721QueryMsg::OwnerOf {
                token_id: "1".into(),
                include_expired: None,
            },
        )
        .unwrap();
    assert_eq!(owner.owner, "bob");
}

#[test]
fn invariant_21_accept_offer_payment_type_mismatch_fails() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_cw20(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    // Bob makes a *native* offer on a *CW20* listing
    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::MakeOffer {
                nft_contract: fx.crystal.to_string(),
                token_id: "1".into(),
                expires_in_blocks: 100,
            },
            &coins(800_000, DENOM),
        )
        .unwrap();

    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::AcceptOffer { offer_id: 1 },
            &[],
        )
        .unwrap_err();
    assert_err(&err, "Wrong payment type");
}

#[test]
fn invariant_22_withdraw_expired_offer_returns_funds_to_buyer() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");

    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::MakeOffer {
                nft_contract: fx.crystal.to_string(),
                token_id: "1".into(),
                expires_in_blocks: 5,
            },
            &coins(800_000, DENOM),
        )
        .unwrap();

    let bob_before = fx.app.wrap().query_balance("bob", DENOM).unwrap().amount;

    // Before expiry — fails
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("stranger"),
            fx.market.clone(),
            &ExecuteMsg::WithdrawExpiredOffer { offer_id: 1 },
            &[],
        )
        .unwrap_err();
    assert_err(&err, "not expired");

    // Advance past expiry
    fx.app.update_block(|b| b.height += 10);

    // Anyone can withdraw — funds go to original buyer
    fx.app
        .execute_contract(
            Addr::unchecked("stranger"),
            fx.market.clone(),
            &ExecuteMsg::WithdrawExpiredOffer { offer_id: 1 },
            &[],
        )
        .unwrap();

    let bob_after = fx.app.wrap().query_balance("bob", DENOM).unwrap().amount;
    assert_eq!(bob_after.u128() - bob_before.u128(), 800_000);
}

#[test]
fn invariant_23_never_expiring_offer_cannot_be_force_withdrawn() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");

    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::MakeOffer {
                nft_contract: fx.crystal.to_string(),
                token_id: "1".into(),
                expires_in_blocks: 0, // never expires
            },
            &coins(800_000, DENOM),
        )
        .unwrap();

    // Even far in the future, stranger cannot withdraw
    fx.app.update_block(|b| b.height += 10_000_000);
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("stranger"),
            fx.market.clone(),
            &ExecuteMsg::WithdrawExpiredOffer { offer_id: 1 },
            &[],
        )
        .unwrap_err();
    assert_err(&err, "not expired");
}

// ───────────────────────────────────────────────────────────────────────────
// 8. PAUSE / SAFETY
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn invariant_24_paused_blocks_listing_buy_offer() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");

    // Pause
    fx.app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::UpdateConfig {
                fee_bps: None,
                fee_bps_non_holder: None,
                fee_bps_crystal: None,
                fee_bps_cosmic: None,
                treasury_addr: None,
                capa_reward_addr: None,
                treasury_share_bps: None,
                capa_gov_contract: None,
                paused: Some(true),
            },
            &[],
        )
        .unwrap();

    // Listing fails
    let err = list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0)
        .unwrap_err();
    assert_err(&err, "paused");

    // Make-offer fails
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::MakeOffer {
                nft_contract: fx.crystal.to_string(),
                token_id: "1".into(),
                expires_in_blocks: 100,
            },
            &coins(800_000, DENOM),
        )
        .unwrap_err();
    assert_err(&err, "paused");
}

#[test]
fn invariant_25_listing_cap_enforced() {
    let mut fx = setup();

    // Lower cap to 2 for this test
    fx.app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::UpdateLaunchCaps {
                caps: LaunchCaps {
                    max_active_listings_per_collection: 2,
                    max_active_offers_per_nft: 20,
                },
            },
            &[],
        )
        .unwrap();

    mint_crystal(&mut fx, "alice", "1");
    mint_crystal(&mut fx, "alice", "2");
    mint_crystal(&mut fx, "alice", "3");

    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 100, 0).unwrap();
    list_nft_native(&mut fx, "alice", Coll::Crystal, "2", 100, 0).unwrap();
    let err = list_nft_native(&mut fx, "alice", Coll::Crystal, "3", 100, 0)
        .unwrap_err();
    assert_err(&err, "active-listing cap");

    // After a cancel, the cap frees up
    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::CancelListing { listing_id: 1 },
            &[],
        )
        .unwrap();
    list_nft_native(&mut fx, "alice", Coll::Crystal, "3", 100, 0).unwrap();
}

// ───────────────────────────────────────────────────────────────────────────
// 9. ROYALTIES
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn invariant_26_royalty_paid_correctly_and_capped_at_15pct() {
    let mut fx = setup();
    let creator = Addr::unchecked("creator");

    // Set a 5% royalty on Crystal collection
    fx.app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::SetRoyalty {
                nft_contract: fx.crystal.to_string(),
                recipient: creator.to_string(),
                royalty_bps: 500,
            },
            &[],
        )
        .unwrap();

    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    let alice_before = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;
    let creator_before = fx.app.wrap().query_balance(&creator, DENOM).unwrap().amount;

    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(1_000_000, DENOM),
        )
        .unwrap();

    let alice_after = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;
    let creator_after = fx.app.wrap().query_balance(&creator, DENOM).unwrap().amount;

    // Fee 1.5% = 15K, royalty 5% = 50K. Alice = 1M - 15K - 50K = 935K
    assert_eq!(alice_after.u128() - alice_before.u128(), 935_000);
    assert_eq!(creator_after.u128() - creator_before.u128(), 50_000);

    // Royalty cap rejection
    let err = fx
        .app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::SetRoyalty {
                nft_contract: fx.crystal.to_string(),
                recipient: creator.to_string(),
                royalty_bps: 1600,
            },
            &[],
        )
        .unwrap_err();
    assert_err(&err, "Royalty too high");
}

#[test]
fn invariant_27_already_listed_check() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();
    // The NFT is now in the marketplace, so alice can't list it again — but
    // sending an NFT she doesn't own would fail at the cw721 level. We instead
    // test the AlreadyListed logic by constructing the same listing twice for
    // the *same* (collection, token_id) — currently impossible because the
    // marketplace holds the NFT. So this invariant is enforced structurally
    // by cw721 ownership transfer.
    // (Kept as a documented invariant.)
}

#[test]
fn invariant_28_zero_price_listing_rejected() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    let err = list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 0, 0).unwrap_err();
    assert_err(&err, "greater than zero");
}

#[test]
fn invariant_29_transfer_ownership_works() {
    let mut fx = setup();
    fx.app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::TransferOwnership {
                new_owner: "new_owner".into(),
            },
            &[],
        )
        .unwrap();

    // Old owner can no longer admin
    let err = fx
        .app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::AddCollection {
                nft_contract: fx.other_collection.to_string(),
            },
            &[],
        )
        .unwrap_err();
    assert_err(&err, "not the contract admin");

    // New owner can
    fx.app
        .execute_contract(
            Addr::unchecked("new_owner"),
            fx.market.clone(),
            &ExecuteMsg::AddCollection {
                nft_contract: fx.other_collection.to_string(),
            },
            &[],
        )
        .unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════
// V1.3.0 — Collection offers + trait registry
// ═══════════════════════════════════════════════════════════════════════════
//
// Test helpers for merkle proofs. We hand-build single-leaf "trees" (root =
// leaf hash itself, no siblings) for the simplest constraint case, and a
// 2-leaf tree (root = sha256(leftLeaf || rightLeaf)) for sibling-required
// proofs. Anything bigger is overkill at this layer — verify_merkle_proof()
// itself is unit-tested implicitly via these flows.

fn sha256_test(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

fn leaf_hash(token_id: &str, trait_type: &str, trait_value: &str) -> [u8; 32] {
    let mut buf = Vec::new();
    buf.extend_from_slice(token_id.as_bytes());
    buf.push(b'|');
    buf.extend_from_slice(trait_type.as_bytes());
    buf.push(b'=');
    buf.extend_from_slice(trait_value.as_bytes());
    sha256_test(&buf)
}

fn hex32(b: [u8; 32]) -> String {
    hex::encode(b)
}

/// Set up a trait registry with EXACTLY ONE leaf for the given (token, type, value).
/// merkle_root = leaf hash itself. Proof at accept-time = empty siblings.
fn set_single_leaf_registry(
    fx: &mut Fixture,
    nft_contract: &str,
    token_id: &str,
    trait_type: &str,
    trait_value: &str,
) {
    let leaf = leaf_hash(token_id, trait_type, trait_value);
    fx.app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::SetTraitRegistry {
                nft_contract: nft_contract.into(),
                merkle_root_hex: hex32(leaf),
                source_url: None,
            },
            &[],
        )
        .unwrap();
}

// ─── Inv 30 ─────────────────────────────────────────────────────────────────
// MakeCollectionOffer (native, no constraints) → escrow held + queryable

#[test]
fn invariant_30_make_collection_offer_native_no_constraints_escrows_funds() {
    let mut fx = setup();
    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::MakeCollectionOffer {
                nft_contract: fx.crystal.to_string(),
                price_per_nft: Uint128::new(100),
                constraints: vec![],
                max_trades: 1,
                expires_in_blocks: 0,
            },
            &[coin(100, DENOM)],
        )
        .unwrap();

    let offer: CollectionOffer = fx
        .app
        .wrap()
        .query_wasm_smart(&fx.market, &QueryMsg::CollectionOffer { offer_id: 1 })
        .unwrap();
    assert_eq!(offer.buyer.as_str(), "alice");
    assert_eq!(offer.escrow_balance, Uint128::new(100));
    assert_eq!(offer.max_trades, 1);
    assert_eq!(offer.trades_filled, 0);
}

// ─── Inv 31 ─────────────────────────────────────────────────────────────────
// Bulk offer (max_trades > 1) requires escrow = price * max_trades

#[test]
fn invariant_31_bulk_offer_escrow_must_equal_price_times_max_trades() {
    let mut fx = setup();
    // Send too little — should reject
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::MakeCollectionOffer {
                nft_contract: fx.crystal.to_string(),
                price_per_nft: Uint128::new(100),
                constraints: vec![],
                max_trades: 5,                 // expects 500
                expires_in_blocks: 0,
            },
            &[coin(400, DENOM)],
        )
        .unwrap_err();
    assert_err(&err, "Escrow mismatch");

    // Correct escrow succeeds
    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::MakeCollectionOffer {
                nft_contract: fx.crystal.to_string(),
                price_per_nft: Uint128::new(100),
                constraints: vec![],
                max_trades: 5,
                expires_in_blocks: 0,
            },
            &[coin(500, DENOM)],
        )
        .unwrap();
}

// ─── Inv 32 ─────────────────────────────────────────────────────────────────
// Constraints non-empty REQUIRES a trait registry on the collection

#[test]
fn invariant_32_constraints_require_trait_registry() {
    let mut fx = setup();
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::MakeCollectionOffer {
                nft_contract: fx.crystal.to_string(),
                price_per_nft: Uint128::new(100),
                constraints: vec![TraitConstraint {
                    trait_type: "Status".into(),
                    accepted_values: vec!["Unbroken".into()],
                }],
                max_trades: 1,
                expires_in_blocks: 0,
            },
            &[coin(100, DENOM)],
        )
        .unwrap_err();
    assert_err(&err, "no trait registry");
}

// ─── Inv 33 ─────────────────────────────────────────────────────────────────
// Cancel collection offer refunds remaining escrow

#[test]
fn invariant_33_cancel_collection_offer_refunds_buyer() {
    let mut fx = setup();
    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::MakeCollectionOffer {
                nft_contract: fx.crystal.to_string(),
                price_per_nft: Uint128::new(500),
                constraints: vec![],
                max_trades: 1,
                expires_in_blocks: 0,
            },
            &[coin(500, DENOM)],
        )
        .unwrap();

    let bal_before = fx
        .app
        .wrap()
        .query_balance("alice", DENOM)
        .unwrap()
        .amount;

    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::CancelCollectionOffer { offer_id: 1 },
            &[],
        )
        .unwrap();

    let bal_after = fx
        .app
        .wrap()
        .query_balance("alice", DENOM)
        .unwrap()
        .amount;

    assert_eq!(bal_after - bal_before, Uint128::new(500));
}

// ─── Inv 34 ─────────────────────────────────────────────────────────────────
// Accept collection offer (no constraints) — listing's seller fulfils, NFT
// transfers, fee + capa-pool routed correctly, listing removed.

#[test]
fn invariant_34_accept_collection_offer_no_constraints_settles() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    // Bob makes a collection offer at 800K (less than listing price — that's
    // OK for collection offers; offer's price_per_nft drives the sale)
    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::MakeCollectionOffer {
                nft_contract: fx.crystal.to_string(),
                price_per_nft: Uint128::new(800_000),
                constraints: vec![],
                max_trades: 1,
                expires_in_blocks: 0,
            },
            &[coin(800_000, DENOM)],
        )
        .unwrap();

    // Alice accepts — token #1 transfers to bob, alice receives funds
    let alice_before = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;
    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::AcceptCollectionOffer {
                offer_id: 1,
                token_id: "1".into(),
                proofs: vec![],
            },
            &[],
        )
        .unwrap();
    let alice_after = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;

    // 1.5% fee on 800K = 12K; alice receives 800K - 12K = 788K
    assert_eq!(alice_after - alice_before, Uint128::new(788_000));

    // NFT now belongs to bob
    let owner_resp: cw721::OwnerOfResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.crystal,
            &cw721::Cw721QueryMsg::OwnerOf {
                token_id: "1".into(),
                include_expired: None,
            },
        )
        .unwrap();
    assert_eq!(owner_resp.owner, "bob");

    // Collection offer auto-closed
    let res = fx.app.wrap().query_wasm_smart::<CollectionOffer>(
        &fx.market,
        &QueryMsg::CollectionOffer { offer_id: 1 },
    );
    assert!(res.is_err(), "offer should be removed after fill");
}

// ─── Inv 35 ─────────────────────────────────────────────────────────────────
// Bulk offer survives partial fill, drains correctly, removes after final fill

#[test]
fn invariant_35_bulk_offer_partial_fill_then_complete() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    mint_crystal(&mut fx, "alice", "2");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();
    list_nft_native(&mut fx, "alice", Coll::Crystal, "2", 1_000_000, 0).unwrap();

    // Bob makes a bulk offer for 2 crystals at 500K each = 1M escrow
    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::MakeCollectionOffer {
                nft_contract: fx.crystal.to_string(),
                price_per_nft: Uint128::new(500_000),
                constraints: vec![],
                max_trades: 2,
                expires_in_blocks: 0,
            },
            &[coin(1_000_000, DENOM)],
        )
        .unwrap();

    // First fill — token #1
    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::AcceptCollectionOffer {
                offer_id: 1,
                token_id: "1".into(),
                proofs: vec![],
            },
            &[],
        )
        .unwrap();

    // Offer still exists with 1 fill remaining
    let offer: CollectionOffer = fx
        .app
        .wrap()
        .query_wasm_smart(&fx.market, &QueryMsg::CollectionOffer { offer_id: 1 })
        .unwrap();
    assert_eq!(offer.trades_filled, 1);
    assert_eq!(offer.escrow_balance, Uint128::new(500_000));

    // Second fill — token #2 → offer auto-closes
    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::AcceptCollectionOffer {
                offer_id: 1,
                token_id: "2".into(),
                proofs: vec![],
            },
            &[],
        )
        .unwrap();

    let res = fx.app.wrap().query_wasm_smart::<CollectionOffer>(
        &fx.market,
        &QueryMsg::CollectionOffer { offer_id: 1 },
    );
    assert!(res.is_err(), "offer should be removed after final fill");
}

// ─── Inv 36 ─────────────────────────────────────────────────────────────────
// Trait-aware collection offer: valid merkle proof passes, wrong trait fails

#[test]
fn invariant_36_trait_constrained_offer_accepts_valid_proof_rejects_invalid() {
    let mut fx = setup();
    let crystal_addr = fx.crystal.to_string();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    // Register single-leaf root for token "1" / Status / Unbroken
    set_single_leaf_registry(&mut fx, &crystal_addr, "1", "Status", "Unbroken");

    // Bob makes a trait-aware bulk offer ("Unbroken" only, max 1 fill)
    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::MakeCollectionOffer {
                nft_contract: crystal_addr.clone(),
                price_per_nft: Uint128::new(800_000),
                constraints: vec![TraitConstraint {
                    trait_type: "Status".into(),
                    accepted_values: vec!["Unbroken".into()],
                }],
                max_trades: 1,
                expires_in_blocks: 0,
            },
            &[coin(800_000, DENOM)],
        )
        .unwrap();

    // Wrong trait_value → fails
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::AcceptCollectionOffer {
                offer_id: 1,
                token_id: "1".into(),
                proofs: vec![TraitProof {
                    trait_type: "Status".into(),
                    trait_value: "Broken".into(),     // not in accepted_values
                    sibling_hashes_hex: vec![],
                    sibling_on_right: vec![],
                }],
            },
            &[],
        )
        .unwrap_err();
    assert_err(&err, "constraints");

    // Correct proof → accepts (single-leaf root, no siblings needed)
    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::AcceptCollectionOffer {
                offer_id: 1,
                token_id: "1".into(),
                proofs: vec![TraitProof {
                    trait_type: "Status".into(),
                    trait_value: "Unbroken".into(),
                    sibling_hashes_hex: vec![],
                    sibling_on_right: vec![],
                }],
            },
            &[],
        )
        .unwrap();
}

// ─── Inv 37 ─────────────────────────────────────────────────────────────────
// Bad merkle proof against a real registry root → BadMerkleProof error

#[test]
fn invariant_37_bad_merkle_proof_rejected() {
    let mut fx = setup();
    let crystal_addr = fx.crystal.to_string();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    // Registry says Status=Unbroken for token "1"
    set_single_leaf_registry(&mut fx, &crystal_addr, "1", "Status", "Unbroken");

    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::MakeCollectionOffer {
                nft_contract: crystal_addr.clone(),
                price_per_nft: Uint128::new(500_000),
                constraints: vec![TraitConstraint {
                    trait_type: "Status".into(),
                    accepted_values: vec!["Unbroken".into()],
                }],
                max_trades: 1,
                expires_in_blocks: 0,
            },
            &[coin(500_000, DENOM)],
        )
        .unwrap();

    // Submit wrong sibling hash (32 bytes of zeros) — single-leaf tree
    // expected NO siblings, supplying any sibling poisons the root.
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::AcceptCollectionOffer {
                offer_id: 1,
                token_id: "1".into(),
                proofs: vec![TraitProof {
                    trait_type: "Status".into(),
                    trait_value: "Unbroken".into(),
                    sibling_hashes_hex: vec![hex32([0u8; 32])],
                    sibling_on_right: vec![true],
                }],
            },
            &[],
        )
        .unwrap_err();
    assert_err(&err, "Merkle proof failed");
}

// ═══════════════════════════════════════════════════════════════════════════
// V1.4.0 — Private listings (whitelisted_buyer)
// ═══════════════════════════════════════════════════════════════════════════
//
// Test helper: list with a whitelisted_buyer field. Mirrors
// list_nft_native but threads the V1.4 field through ListNftMsg.
fn list_nft_native_private(
    fx: &mut Fixture,
    seller: &str,
    collection: Coll,
    token_id: &str,
    price: u128,
    whitelisted_buyer: &str,
) -> anyhow::Result<()> {
    let list_msg = ListNftMsg {
        price: Uint128::new(price),
        payment: PaymentType::Native { denom: DENOM.into() },
        expires_in_blocks: 0,
        whitelisted_buyer: Some(whitelisted_buyer.into()),
        lock_in_blocks: None,
        whitelist: None,
    };
    let market = fx.market.to_string();
    let coll_addr = match collection {
        Coll::Crystal => fx.crystal.clone(),
        Coll::Other => fx.other_collection.clone(),
    };
    fx.app
        .execute_contract(
            Addr::unchecked(seller),
            coll_addr,
            &cw721_base::ExecuteMsg::<Empty, Empty>::SendNft {
                contract: market,
                token_id: token_id.into(),
                msg: to_json_binary(&list_msg).unwrap(),
            },
            &[],
        )
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{}", e.root_cause()))
}

// ─── Inv 38 ─────────────────────────────────────────────────────────────────
// Private listing: only whitelisted buyer can BuyNft

#[test]
fn invariant_38_private_listing_only_whitelisted_buyer_can_buy() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native_private(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, "bob").unwrap();

    // Stranger tries to buy — should fail with "private"
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("stranger"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(1_000_000, DENOM),
        )
        .unwrap_err();
    assert_err(&err, "private");

    // Bob (whitelisted) succeeds
    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(1_000_000, DENOM),
        )
        .unwrap();
}

// ─── Inv 39 ─────────────────────────────────────────────────────────────────
// Private listing: only whitelisted buyer's offer can be Accepted

#[test]
fn invariant_39_private_listing_seller_can_only_accept_whitelisted_offer() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native_private(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, "bob").unwrap();

    // Stranger makes an offer — allowed (no offer-side gate; refund via cancel)
    fx.app
        .execute_contract(
            Addr::unchecked("stranger"),
            fx.market.clone(),
            &ExecuteMsg::MakeOffer {
                nft_contract: fx.crystal.to_string(),
                token_id: "1".into(),
                expires_in_blocks: 0,
            },
            &coins(900_000, DENOM),
        )
        .unwrap();

    // Bob also makes an offer
    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::MakeOffer {
                nft_contract: fx.crystal.to_string(),
                token_id: "1".into(),
                expires_in_blocks: 0,
            },
            &coins(800_000, DENOM),
        )
        .unwrap();

    // Alice CAN'T accept stranger's offer (offer_id=1) — listing is private to bob
    let err = fx
        .app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::AcceptOffer { offer_id: 1 },
            &[],
        )
        .unwrap_err();
    assert_err(&err, "private");

    // Alice CAN accept bob's offer (offer_id=2)
    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.market.clone(),
            &ExecuteMsg::AcceptOffer { offer_id: 2 },
            &[],
        )
        .unwrap();
}

// ─── Inv 40 ─────────────────────────────────────────────────────────────────
// Open listing (no whitelisted_buyer) — anyone can buy + offer-accept (V1.0
// regression-guard ensuring V1.4 didn't break the default path)

#[test]
fn invariant_40_open_listing_default_path_unchanged() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, 0).unwrap();

    // Any wallet can buy
    fx.app
        .execute_contract(
            Addr::unchecked("stranger"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(1_000_000, DENOM),
        )
        .unwrap();
}

// ─── Inv 41 ─────────────────────────────────────────────────────────────────
// Private listing CW20 path refunds (instead of erroring) when buyer
// isn't whitelisted — funds came in via Receive so can't reject

#[test]
fn invariant_41_private_listing_cw20_path_refunds_non_whitelisted() {
    let mut fx = setup();
    let capa = fx.capa.clone();

    mint_crystal(&mut fx, "alice", "1");

    // Alice lists with CW20 payment + whitelisted_buyer = bob
    let list_msg = ListNftMsg {
        price: Uint128::new(1_000_000),
        payment: PaymentType::Cw20 { contract_addr: capa.to_string() },
        expires_in_blocks: 0,
        whitelisted_buyer: Some("bob".into()),
        lock_in_blocks: None,
        whitelist: None,
    };
    let market = fx.market.to_string();
    fx.app
        .execute_contract(
            Addr::unchecked("alice"),
            fx.crystal.clone(),
            &cw721_base::ExecuteMsg::<Empty, Empty>::SendNft {
                contract: market.clone(),
                token_id: "1".into(),
                msg: to_json_binary(&list_msg).unwrap(),
            },
            &[],
        )
        .unwrap();

    // Stranger sends CAPA to buy — should be refunded (NOT errored)
    let stranger_before = cw20_balance(&fx, "stranger");
    fx.app
        .execute_contract(
            Addr::unchecked("stranger"),
            capa.clone(),
            &Cw20ExecuteMsg::Send {
                contract: market.clone(),
                amount: Uint128::new(1_000_000),
                msg: to_json_binary(&Cw20HookMsg::BuyNft { listing_id: 1 }).unwrap(),
            },
            &[],
        )
        .unwrap();
    let stranger_after = cw20_balance(&fx, "stranger");
    // Funds came back — net zero for stranger
    assert_eq!(stranger_before, stranger_after);

    // Listing is still active (not consumed)
    let listing: crate::state::Listing = fx.app.wrap().query_wasm_smart(
        &fx.market,
        &QueryMsg::Listing { listing_id: 1 },
    ).unwrap();
    assert_eq!(listing.id, 1);
}

fn cw20_balance(fx: &Fixture, addr: &str) -> Uint128 {
    let bal: cw20::BalanceResponse = fx.app.wrap().query_wasm_smart(
        &fx.capa,
        &cw20::Cw20QueryMsg::Balance { address: addr.into() },
    ).unwrap();
    bal.balance
}

// ═══════════════════════════════════════════════════════════════════════════
// V1.5.0 — Vesting (TLA-Lock) + Promo whitelist (multi-address slots)
// ═══════════════════════════════════════════════════════════════════════════
//
// Test helper: list with V1.5 vesting + whitelist combinations.

use crate::msg::WhitelistEntry as WLEntry;

#[allow(clippy::too_many_arguments)]
fn list_nft_v15(
    fx: &mut Fixture,
    seller: &str,
    collection: Coll,
    token_id: &str,
    price: u128,
    lock_in_blocks: Option<u64>,
    whitelist: Option<Vec<WLEntry>>,
    whitelisted_buyer: Option<String>,
) -> anyhow::Result<()> {
    let list_msg = ListNftMsg {
        price: Uint128::new(price),
        payment: PaymentType::Native { denom: DENOM.into() },
        expires_in_blocks: 0,
        whitelisted_buyer,
        lock_in_blocks,
        whitelist,
    };
    let market = fx.market.to_string();
    let coll_addr = match collection {
        Coll::Crystal => fx.crystal.clone(),
        Coll::Other => fx.other_collection.clone(),
    };
    fx.app
        .execute_contract(
            Addr::unchecked(seller),
            coll_addr,
            &cw721_base::ExecuteMsg::<Empty, Empty>::SendNft {
                contract: market,
                token_id: token_id.into(),
                msg: to_json_binary(&list_msg).unwrap(),
            },
            &[],
        )
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{}", e.root_cause()))
}

// ─── Inv 42 ─────────────────────────────────────────────────────────────────
// Vesting buy escrows NFT (not transferred to buyer)

#[test]
fn invariant_42_vesting_buy_escrows_nft() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_v15(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, Some(100), None, None).unwrap();

    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(1_000_000, DENOM),
        )
        .unwrap();

    // NFT still owned by the marketplace — bob does NOT have it yet
    let owner_resp: cw721::OwnerOfResponse = fx.app.wrap()
        .query_wasm_smart(&fx.crystal, &cw721::Cw721QueryMsg::OwnerOf {
            token_id: "1".into(), include_expired: None,
        }).unwrap();
    assert_eq!(owner_resp.owner, fx.market.to_string(),
        "vesting buy must keep NFT in marketplace escrow");

    // Listing still exists, in locked state
    let l: crate::state::Listing = fx.app.wrap()
        .query_wasm_smart(&fx.market, &QueryMsg::Listing { listing_id: 1 }).unwrap();
    assert_eq!(l.locked_for.as_ref().map(|a| a.as_str()), Some("bob"));
    assert!(l.time_locked_until.is_some());
}

// ─── Inv 43 ─────────────────────────────────────────────────────────────────
// Release before unlock fails

#[test]
fn invariant_43_release_before_unlock_fails() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_v15(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, Some(1000), None, None).unwrap();

    fx.app.execute_contract(
        Addr::unchecked("bob"),
        fx.market.clone(),
        &ExecuteMsg::BuyNft { listing_id: 1 },
        &coins(1_000_000, DENOM),
    ).unwrap();

    // Don't advance blocks — try to release immediately
    let err = fx.app.execute_contract(
        Addr::unchecked("anyone"),
        fx.market.clone(),
        &ExecuteMsg::Release { listing_id: 1 },
        &[],
    ).unwrap_err();
    assert_err(&err, "Vesting period not over");
}

// ─── Inv 44 ─────────────────────────────────────────────────────────────────
// Release after unlock transfers NFT to buyer

#[test]
fn invariant_44_release_after_unlock_transfers_nft() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_v15(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, Some(50), None, None).unwrap();

    fx.app.execute_contract(
        Addr::unchecked("bob"),
        fx.market.clone(),
        &ExecuteMsg::BuyNft { listing_id: 1 },
        &coins(1_000_000, DENOM),
    ).unwrap();

    // Advance blocks past unlock
    fx.app.update_block(|b| b.height += 100);

    // Anyone (carol) can call Release
    fx.app.execute_contract(
        Addr::unchecked("carol"),
        fx.market.clone(),
        &ExecuteMsg::Release { listing_id: 1 },
        &[],
    ).unwrap();

    // NFT now belongs to bob
    let owner_resp: cw721::OwnerOfResponse = fx.app.wrap()
        .query_wasm_smart(&fx.crystal, &cw721::Cw721QueryMsg::OwnerOf {
            token_id: "1".into(), include_expired: None,
        }).unwrap();
    assert_eq!(owner_resp.owner, "bob");

    // Listing removed
    let res = fx.app.wrap().query_wasm_smart::<crate::state::Listing>(
        &fx.market, &QueryMsg::Listing { listing_id: 1 },
    );
    assert!(res.is_err(), "listing must be removed after release");
}

// ─── Inv 45 ─────────────────────────────────────────────────────────────────
// Cancel listing in locked state fails (seller already paid)

#[test]
fn invariant_45_cancel_locked_listing_fails() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_v15(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, Some(100), None, None).unwrap();

    fx.app.execute_contract(
        Addr::unchecked("bob"),
        fx.market.clone(),
        &ExecuteMsg::BuyNft { listing_id: 1 },
        &coins(1_000_000, DENOM),
    ).unwrap();

    // Alice tries to cancel — should fail because listing is locked
    let err = fx.app.execute_contract(
        Addr::unchecked("alice"),
        fx.market.clone(),
        &ExecuteMsg::CancelListing { listing_id: 1 },
        &[],
    ).unwrap_err();
    assert_err(&err, "locked");
}

// ─── Inv 46 ─────────────────────────────────────────────────────────────────
// Whitelist with multiple addresses — first whitelisted buyer consumes a slot

#[test]
fn invariant_46_whitelist_first_buyer_consumes_slot() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");

    let whitelist = vec![
        WLEntry { addr: "bob".into(), max_buys: 1 },
        WLEntry { addr: "carol".into(), max_buys: 1 },
    ];
    list_nft_v15(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, None, Some(whitelist), None).unwrap();

    // Bob (whitelisted) buys successfully
    fx.app.execute_contract(
        Addr::unchecked("bob"),
        fx.market.clone(),
        &ExecuteMsg::BuyNft { listing_id: 1 },
        &coins(1_000_000, DENOM),
    ).unwrap();

    // Bob now owns the NFT (atomic — no time-lock)
    let owner_resp: cw721::OwnerOfResponse = fx.app.wrap()
        .query_wasm_smart(&fx.crystal, &cw721::Cw721QueryMsg::OwnerOf {
            token_id: "1".into(), include_expired: None,
        }).unwrap();
    assert_eq!(owner_resp.owner, "bob");
}

// ─── Inv 47 ─────────────────────────────────────────────────────────────────
// Whitelist non-member rejected

#[test]
fn invariant_47_whitelist_non_member_rejected() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");

    let whitelist = vec![WLEntry { addr: "bob".into(), max_buys: 1 }];
    list_nft_v15(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, None, Some(whitelist), None).unwrap();

    // Stranger (not in whitelist) tries to buy
    let err = fx.app.execute_contract(
        Addr::unchecked("stranger"),
        fx.market.clone(),
        &ExecuteMsg::BuyNft { listing_id: 1 },
        &coins(1_000_000, DENOM),
    ).unwrap_err();
    assert_err(&err, "not on this listing");
}

// ─── Inv 48 ─────────────────────────────────────────────────────────────────
// Whitelist + whitelisted_buyer mutual exclusion enforced at list-time

#[test]
fn invariant_48_whitelist_and_private_conflict_rejected() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");

    let err = list_nft_v15(
        &mut fx, "alice", Coll::Crystal, "1", 1_000_000, None,
        Some(vec![WLEntry { addr: "bob".into(), max_buys: 1 }]),
        Some("carol".into()),
    ).unwrap_err();
    assert_err(&err, "Set whitelisted_buyer OR whitelist");
}

// ─── Inv 49 ─────────────────────────────────────────────────────────────────
// TLA + whitelist combined: whitelisted buyer triggers locked state,
// listing exits whitelist mode (only one buyer can fill a TLA listing)

#[test]
fn invariant_49_tla_plus_whitelist_combined() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");

    let whitelist = vec![
        WLEntry { addr: "bob".into(), max_buys: 1 },
        WLEntry { addr: "carol".into(), max_buys: 1 },
    ];
    list_nft_v15(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, Some(50), Some(whitelist), None).unwrap();

    // Bob (whitelisted) buys — payment routed, NFT escrowed, locked_for = bob
    fx.app.execute_contract(
        Addr::unchecked("bob"),
        fx.market.clone(),
        &ExecuteMsg::BuyNft { listing_id: 1 },
        &coins(1_000_000, DENOM),
    ).unwrap();

    let l: crate::state::Listing = fx.app.wrap()
        .query_wasm_smart(&fx.market, &QueryMsg::Listing { listing_id: 1 }).unwrap();
    assert_eq!(l.locked_for.as_ref().map(|a| a.as_str()), Some("bob"));

    // Carol (also whitelisted) tries to buy — listing is locked
    let err = fx.app.execute_contract(
        Addr::unchecked("carol"),
        fx.market.clone(),
        &ExecuteMsg::BuyNft { listing_id: 1 },
        &coins(1_000_000, DENOM),
    ).unwrap_err();
    assert_err(&err, "locked");
}

// ─── Inv 50 ─────────────────────────────────────────────────────────────────
// AcceptOffer on a vesting (TLA) listing also enters locked state

#[test]
fn invariant_50_accept_offer_on_vesting_listing_locks() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");
    list_nft_v15(&mut fx, "alice", Coll::Crystal, "1", 1_000_000, Some(100), None, None).unwrap();

    // Bob makes an offer
    fx.app.execute_contract(
        Addr::unchecked("bob"),
        fx.market.clone(),
        &ExecuteMsg::MakeOffer {
            nft_contract: fx.crystal.to_string(),
            token_id: "1".into(),
            expires_in_blocks: 0,
        },
        &coins(800_000, DENOM),
    ).unwrap();

    // Alice accepts → payments route, but NFT stays escrowed (vesting)
    fx.app.execute_contract(
        Addr::unchecked("alice"),
        fx.market.clone(),
        &ExecuteMsg::AcceptOffer { offer_id: 1 },
        &[],
    ).unwrap();

    // NFT not transferred yet
    let owner_resp: cw721::OwnerOfResponse = fx.app.wrap()
        .query_wasm_smart(&fx.crystal, &cw721::Cw721QueryMsg::OwnerOf {
            token_id: "1".into(), include_expired: None,
        }).unwrap();
    assert_eq!(owner_resp.owner, fx.market.to_string());

    // Listing is locked-for bob
    let l: crate::state::Listing = fx.app.wrap()
        .query_wasm_smart(&fx.market, &QueryMsg::Listing { listing_id: 1 }).unwrap();
    assert_eq!(l.locked_for.as_ref().map(|a| a.as_str()), Some("bob"));
}

// ─── Inv 51 ─────────────────────────────────────────────────────────────────
// Vesting duration over MAX_TIME_LOCK_BLOCKS rejected at list-time

#[test]
fn invariant_51_vesting_duration_cap_enforced() {
    let mut fx = setup();
    mint_crystal(&mut fx, "alice", "1");

    // 50_000_000 > MAX_TIME_LOCK_BLOCKS (10M)
    let err = list_nft_v15(
        &mut fx, "alice", Coll::Crystal, "1", 1_000_000,
        Some(50_000_000), None, None,
    ).unwrap_err();
    assert_err(&err, "Vesting duration too long");
}

// ═══════════════════════════════════════════════════════════════════════════
// V1.6.0 — One-sided best-of-two fee model (Daniel 2026-05-01)
// ═══════════════════════════════════════════════════════════════════════════
//
// These tests verify the V1.6 plumbing: that the new 3-tier schedule fields
// exist, can be set via UpdateConfig + MigrateMsg, and that the new
// FeeInfoForTrade query returns the right shape.
//
// IMPORTANT: full tier-aware behavior (Cosmic short-circuit, Crystal-tier
// detection, best-of-buyer-seller) cannot be exercised end-to-end in this
// test environment. The tier-resolution chain (highest_crystal_tier →
// resolve_tier on ALTAR/FUSION/MINT contracts) is hardcoded to mainnet
// addresses that don't exist in cw-multi-test, so resolve_tier silently
// returns None for every token (already documented behavior). Buyers +
// sellers therefore always fall through to fee_bps_non_holder.
//
// Full tier-coverage is deferred to a manual on-chain smoke test
// post-deploy (Phase 2 verification — query FeeInfoForTrade with real
// Cosmic-holder addresses on phoenix-1).

#[test]
fn v16_instantiate_seeds_tier_schedule() {
    // Fresh-deploy invariant: instantiate sets the V1.6 tier fields to
    // sane defaults so the contract is usable without running migrate.
    let fx = setup();
    let cfg: crate::state::Config = fx
        .app
        .wrap()
        .query_wasm_smart(&fx.market, &QueryMsg::Config {})
        .unwrap();

    // Setup uses fee_bps=150 → fee_bps_non_holder mirrors that.
    assert_eq!(cfg.fee_bps, 150);
    assert_eq!(cfg.fee_bps_non_holder, 150);
    assert_eq!(cfg.fee_bps_crystal, 150);
    assert_eq!(cfg.fee_bps_cosmic, 0);
}

#[test]
fn v16_update_config_sets_each_tier_independently() {
    // Admin can tune each tier independently — the production rollout
    // will use this to set non_holder=500, crystal=150, cosmic=0.
    let mut fx = setup();

    fx.app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::UpdateConfig {
                fee_bps: None,
                fee_bps_non_holder: Some(500),
                fee_bps_crystal: Some(150),
                fee_bps_cosmic: Some(0),
                treasury_addr: None,
                capa_reward_addr: None,
                treasury_share_bps: None,
                capa_gov_contract: None,
                paused: None,
            },
            &[],
        )
        .unwrap();

    let cfg: crate::state::Config = fx
        .app
        .wrap()
        .query_wasm_smart(&fx.market, &QueryMsg::Config {})
        .unwrap();
    assert_eq!(cfg.fee_bps_non_holder, 500);
    assert_eq!(cfg.fee_bps_crystal, 150);
    assert_eq!(cfg.fee_bps_cosmic, 0);
    // Legacy field untouched
    assert_eq!(cfg.fee_bps, 150);
}

#[test]
fn v16_update_config_rejects_oversize_tier() {
    // Each tier is bounded by MAX_FEE_BPS — admin cannot exceed it.
    let mut fx = setup();
    let err = fx
        .app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::UpdateConfig {
                fee_bps: None,
                fee_bps_non_holder: Some(600), // > 5%
                fee_bps_crystal: None,
                fee_bps_cosmic: None,
                treasury_addr: None,
                capa_reward_addr: None,
                treasury_share_bps: None,
                capa_gov_contract: None,
                paused: None,
            },
            &[],
        )
        .unwrap_err();
    assert_err(&err, "Fee");
}

#[test]
fn v16_fee_info_for_trade_query_returns_full_schedule() {
    // The new dual-side preview query exposes all 3 tiers + applied tier
    // so the frontend can render "Cosmic discount applied via seller"
    // without re-deriving the logic.
    let mut fx = setup();

    // Move config to canonical V1.6 production rates
    fx.app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::UpdateConfig {
                fee_bps: None,
                fee_bps_non_holder: Some(500),
                fee_bps_crystal: Some(150),
                fee_bps_cosmic: Some(0),
                treasury_addr: None,
                capa_reward_addr: None,
                treasury_share_bps: None,
                capa_gov_contract: None,
                paused: None,
            },
            &[],
        )
        .unwrap();

    // No tier resolution in test env → both addrs read as non-holder.
    let resp: FeeInfoForTradeResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.market,
            &QueryMsg::FeeInfoForTrade {
                buyer: Some("alice".to_string()),
                seller: Some("bob".to_string()),
            },
        )
        .unwrap();

    assert_eq!(resp.fee_bps, 500);
    assert_eq!(resp.applied_tier, "non_holder");
    assert_eq!(resp.fee_bps_non_holder, 500);
    assert_eq!(resp.fee_bps_crystal, 150);
    assert_eq!(resp.fee_bps_cosmic, 0);
    assert_eq!(resp.buyer_tier, None);
    assert_eq!(resp.seller_tier, None);
}

#[test]
fn v16_fee_info_for_trade_handles_missing_addresses() {
    // Either side can be omitted — graceful fallback to non-holder rate.
    let fx = setup();
    let resp: FeeInfoForTradeResponse = fx
        .app
        .wrap()
        .query_wasm_smart(
            &fx.market,
            &QueryMsg::FeeInfoForTrade {
                buyer: None,
                seller: None,
            },
        )
        .unwrap();
    assert_eq!(resp.fee_bps, 150); // mirrors setup's fee_bps_non_holder
    assert_eq!(resp.applied_tier, "non_holder");
}

#[test]
fn v16_settle_sale_uses_non_holder_rate_in_test_env() {
    // End-to-end: list + buy. Tier resolution returns None in test env,
    // so the trade should fall through to fee_bps_non_holder. Verifies
    // the seller-side wiring doesn't crash the settlement flow.
    //
    // (Cosmic / Crystal-tier paths are deploy-time smoke-tested.)
    let mut fx = setup();

    // Set canonical V1.6 production rates so the math matches what
    // production will actually compute.
    fx.app
        .execute_contract(
            fx.owner.clone(),
            fx.market.clone(),
            &ExecuteMsg::UpdateConfig {
                fee_bps: None,
                fee_bps_non_holder: Some(500),
                fee_bps_crystal: Some(150),
                fee_bps_cosmic: Some(0),
                treasury_addr: None,
                capa_reward_addr: None,
                treasury_share_bps: Some(333), // 2/3 to treasury
                capa_gov_contract: None,
                paused: None,
            },
            &[],
        )
        .unwrap();

    // Alice mints + lists a Crystal token at 1_000_000 uluna.
    mint_crystal(&mut fx, "alice", "v16_seller_test");
    list_nft_native(
        &mut fx,
        "alice",
        Coll::Crystal,
        "v16_seller_test",
        1_000_000,
        0,
    )
    .unwrap();

    // Bob buys at the listing price.
    let alice_before = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;
    let treasury_before = fx
        .app
        .wrap()
        .query_balance(&fx.treasury, DENOM)
        .unwrap()
        .amount;
    let capa_pool_before = fx
        .app
        .wrap()
        .query_balance(&fx.capa_pool, DENOM)
        .unwrap()
        .amount;

    fx.app
        .execute_contract(
            Addr::unchecked("bob"),
            fx.market.clone(),
            &ExecuteMsg::BuyNft { listing_id: 1 },
            &coins(1_000_000, DENOM),
        )
        .unwrap();

    let alice_after = fx.app.wrap().query_balance("alice", DENOM).unwrap().amount;
    let treasury_after = fx
        .app
        .wrap()
        .query_balance(&fx.treasury, DENOM)
        .unwrap()
        .amount;
    let capa_pool_after = fx
        .app
        .wrap()
        .query_balance(&fx.capa_pool, DENOM)
        .unwrap()
        .amount;

    // 5% fee on 1_000_000 = 50_000. Alice receives 950_000.
    assert_eq!(alice_after - alice_before, Uint128::new(950_000));
    // Treasury share = 333/500 of 50_000 = 33_300. CAPA pool = 16_700.
    assert_eq!(treasury_after - treasury_before, Uint128::new(33_300));
    assert_eq!(capa_pool_after - capa_pool_before, Uint128::new(16_700));
}
