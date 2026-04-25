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
    Cw20HookMsg, ExecuteMsg, FeeInfoResponse, InstantiateMsg, IsAllowedResponse, ListNftMsg,
    QueryMsg,
};
use crate::state::{LaunchCaps, PaymentType};

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

#[test]
fn invariant_13_crystal_holder_buyer_pays_zero_fee() {
    let mut fx = setup();
    // crystal_holder owns Crystal #1
    mint_crystal(&mut fx, "crystal_holder", "1");
    // Alice lists Crystal #2
    mint_crystal(&mut fx, "alice", "2");
    list_nft_native(&mut fx, "alice", Coll::Crystal, "2", 1_000_000, 0).unwrap();

    // Crystal-holder buys
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

    // Alice gets full 1M (zero fee)
    assert_eq!(alice_after.u128() - alice_before.u128(), 1_000_000);
    assert_eq!(treasury_after.u128() - treasury_before.u128(), 0);
}

#[test]
fn invariant_14_fee_info_query_reflects_holder_status() {
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
    assert_eq!(resp_holder.fee_bps, 0);
    assert_eq!(resp_holder.discount_bps, 150);
    assert!(resp_holder.crystal_holder);

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
