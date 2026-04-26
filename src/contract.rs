#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Addr, BankMsg, Binary, Coin, CosmosMsg, Deps, DepsMut, Env,
    MessageInfo, Order, Response, StdResult, Uint128, WasmMsg, from_json,
};
use cw2::set_contract_version;
use cw20::Cw20ExecuteMsg;
use cw721::TokensResponse;

use crate::error::ContractError;
use crate::msg::{
    AllowedCollectionsResponse, CollectionStatsResponse, Cw20HookMsg, ExecuteMsg,
    FeeInfoResponse, InstantiateMsg, IsAllowedResponse, ListNftMsg, ListingsResponse,
    MigrateMsg, OffersResponse, QueryMsg, RoyaltyInfoResponse,
};
use crate::state::{
    Config, LaunchCaps, Listing, Offer, PaymentType, RoyaltyInfo,
    ACTIVE_LISTING, ACTIVE_LISTINGS_PER_COLLECTION, ACTIVE_OFFERS_PER_NFT,
    ALLOWED_COLLECTIONS, CONFIG, CRYSTAL_NFT_CONTRACT, FEE_DISCOUNT_TIERS,
    LAUNCH_CAPS, LISTING_COUNT, LISTINGS, OFFER_COUNT, OFFERS, OFFERS_BY_NFT,
    ROYALTIES,
};

const CONTRACT_NAME: &str = "crates.io:atrium-marketplace";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_LIMIT: u32 = 30;
const DEFAULT_LIMIT: u32 = 10;
const MAX_ROYALTY_BPS: u16 = 1500; // 15%
const MAX_FEE_BPS: u16 = 500; // 5%

// ═══════════════════════════════════════════
// MIGRATE — V1.0 → V1.1 (tier ladder) → V1.2 (Cosmic-only)
// ═══════════════════════════════════════════
//
// V1.2.0 changes (2026-04-26, Daniel):
//   • Tier ladder collapsed — only "cosmic" gets a discount.
//   • All other tiers (prismatic/radiant/charged/raw) AND non-holders
//     pay config.fee_bps (default 1.5%, will bump to 5.0% after operator
//     notice to existing sellers — see plan_atrium_v1_2_post_migrate.md).
//   • highest_crystal_tier() unchanged — still surfaces highest tier
//     name for FeeInfoResponse so UI can show "you own a Charged
//     Crystal" badges, even though only Cosmic gets fee discount.
//
// V1.1.0 changes (deprecated, kept for code archaeology):
//   • Crystal-holder discount split into a 5-rung tier ladder
//     (Cosmic 0% / Prismatic 0.25% / Radiant 0.50% / Charged 1.0% / Raw 1.5%)
//   • Resolution chain: ALTAR → FUSION → MINT (hardcoded mainnet addrs)
//   • Up to 30 Crystals scanned per buyer to bound gas
//   • FeeInfoResponse extended with `crystal_tier: Option<String>`
//
// No state changes in any migration — same Config, same Listings, same
// Offers. migrate() only updates the cw2 contract version marker so
// external indexers know which version is running. fee_bps is changed
// separately via execute_update_config (NOT in migrate) to keep the
// in-flight-listing-impact decision in operator hands.

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    Ok(Response::new()
        .add_attribute("action", "migrate")
        .add_attribute("new_version", CONTRACT_VERSION))
}

// ═══════════════════════════════════════════
// INSTANTIATE
// ═══════════════════════════════════════════

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    if msg.fee_bps > MAX_FEE_BPS {
        return Err(ContractError::FeeTooHigh { bps: msg.fee_bps });
    }

    // AUDIT FIX: treasury share cannot exceed total fee
    if msg.treasury_share_bps > msg.fee_bps {
        return Err(ContractError::TreasuryShareTooHigh {});
    }

    let config = Config {
        owner: info.sender.clone(),
        fee_bps: msg.fee_bps,
        treasury_addr: deps.api.addr_validate(&msg.treasury_addr)?,
        capa_reward_addr: deps.api.addr_validate(&msg.capa_reward_addr)?,
        treasury_share_bps: msg.treasury_share_bps,
        capa_gov_contract: msg
            .capa_gov_contract
            .map(|a| deps.api.addr_validate(&a))
            .transpose()?,
        paused: false,
    };

    CONFIG.save(deps.storage, &config)?;
    LISTING_COUNT.save(deps.storage, &0u64)?;
    OFFER_COUNT.save(deps.storage, &0u64)?;

    // Crystal NFT contract — used for the holder-discount lookup.
    let crystal = deps.api.addr_validate(&msg.crystal_nft_contract)?;
    CRYSTAL_NFT_CONTRACT.save(deps.storage, &crystal)?;

    // Launch caps — anti-DoS rails for the first 30 days.
    LAUNCH_CAPS.save(deps.storage, &msg.launch_caps)?;

    // Initial allowlisted collections (deduped by Map's idempotent save).
    for c in &msg.initial_collections {
        let addr = deps.api.addr_validate(c)?;
        ALLOWED_COLLECTIONS.save(deps.storage, addr.as_str(), &())?;
    }

    Ok(Response::new()
        .add_attribute("action", "instantiate")
        .add_attribute("owner", info.sender)
        .add_attribute("fee_bps", msg.fee_bps.to_string())
        .add_attribute("crystal_nft", crystal)
        .add_attribute("initial_collections", msg.initial_collections.len().to_string()))
}

// ═══════════════════════════════════════════
// EXECUTE
// ═══════════════════════════════════════════

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::ReceiveNft(cw721_msg) => execute_receive_nft(deps, env, info, cw721_msg),
        ExecuteMsg::Receive(cw20_msg) => execute_receive_cw20(deps, env, info, cw20_msg),
        ExecuteMsg::BuyNft { listing_id } => execute_buy_native(deps, env, info, listing_id),
        ExecuteMsg::CancelListing { listing_id } => execute_cancel_listing(deps, info, listing_id),
        ExecuteMsg::MakeOffer {
            nft_contract,
            token_id,
            expires_in_blocks,
        } => execute_make_offer_native(deps, env, info, nft_contract, token_id, expires_in_blocks),
        ExecuteMsg::AcceptOffer { offer_id } => execute_accept_offer(deps, env, info, offer_id),
        ExecuteMsg::CancelOffer { offer_id } => execute_cancel_offer(deps, info, offer_id),
        ExecuteMsg::WithdrawExpiredOffer { offer_id } => {
            execute_withdraw_expired(deps, env, info, offer_id)
        }
        ExecuteMsg::SetRoyalty {
            nft_contract,
            recipient,
            royalty_bps,
        } => execute_set_royalty(deps, info, nft_contract, recipient, royalty_bps),
        ExecuteMsg::UpdateConfig {
            fee_bps,
            treasury_addr,
            capa_reward_addr,
            treasury_share_bps,
            capa_gov_contract,
            paused,
        } => execute_update_config(
            deps,
            info,
            fee_bps,
            treasury_addr,
            capa_reward_addr,
            treasury_share_bps,
            capa_gov_contract,
            paused,
        ),
        ExecuteMsg::AddCollection { nft_contract } => {
            execute_add_collection(deps, info, nft_contract)
        }
        ExecuteMsg::RemoveCollection { nft_contract } => {
            execute_remove_collection(deps, info, nft_contract)
        }
        ExecuteMsg::SetCrystalContract { nft_contract } => {
            execute_set_crystal_contract(deps, info, nft_contract)
        }
        ExecuteMsg::UpdateLaunchCaps { caps } => execute_update_launch_caps(deps, info, caps),
        ExecuteMsg::TransferOwnership { new_owner } => {
            execute_transfer_ownership(deps, info, new_owner)
        }
    }
}

// ───── CW721 Receive: List NFT ─────

fn execute_receive_nft(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    cw721_msg: cw721::Cw721ReceiveMsg,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if config.paused {
        return Err(ContractError::Paused {});
    }

    let nft_contract = info.sender.clone(); // The CW721 contract that sent the NFT
    let seller = deps.api.addr_validate(&cw721_msg.sender)?;
    let token_id = cw721_msg.token_id;

    // ─── Curation gate ───
    if !ALLOWED_COLLECTIONS.has(deps.storage, nft_contract.as_str()) {
        return Err(ContractError::CollectionNotAllowed {
            addr: nft_contract.to_string(),
        });
    }

    // ─── Listing-cap gate (anti-DoS) ───
    let caps = LAUNCH_CAPS.load(deps.storage)?;
    if caps.max_active_listings_per_collection > 0 {
        let current = ACTIVE_LISTINGS_PER_COLLECTION
            .may_load(deps.storage, nft_contract.as_str())?
            .unwrap_or(0);
        if current >= caps.max_active_listings_per_collection {
            return Err(ContractError::ListingCapExceeded {
                cap: caps.max_active_listings_per_collection,
            });
        }
    }

    // Parse the embedded listing message
    let list_msg: ListNftMsg =
        from_json(&cw721_msg.msg).map_err(|_| ContractError::InvalidCw721Msg {})?;

    if list_msg.price.is_zero() {
        return Err(ContractError::ZeroPrice {});
    }

    // Check no active listing exists for this NFT
    if ACTIVE_LISTING
        .may_load(deps.storage, (nft_contract.as_str(), &token_id))?
        .is_some()
    {
        return Err(ContractError::AlreadyListed {});
    }

    // Create listing
    let id = LISTING_COUNT.load(deps.storage)? + 1;
    LISTING_COUNT.save(deps.storage, &id)?;

    let expires_at = if list_msg.expires_in_blocks > 0 {
        env.block.height + list_msg.expires_in_blocks
    } else {
        0
    };

    let listing = Listing {
        id,
        seller: seller.clone(),
        nft_contract: nft_contract.clone(),
        token_id: token_id.clone(),
        price: list_msg.price,
        payment: list_msg.payment.clone(),
        expires_at,
        created_at: env.block.height,
    };

    LISTINGS.save(deps.storage, id, &listing)?;
    ACTIVE_LISTING.save(deps.storage, (nft_contract.as_str(), &token_id), &id)?;

    // Bump active-listings counter for this collection
    let new_count = ACTIVE_LISTINGS_PER_COLLECTION
        .may_load(deps.storage, nft_contract.as_str())?
        .unwrap_or(0)
        .saturating_add(1);
    ACTIVE_LISTINGS_PER_COLLECTION.save(deps.storage, nft_contract.as_str(), &new_count)?;

    Ok(Response::new()
        .add_attribute("action", "list_nft")
        .add_attribute("listing_id", id.to_string())
        .add_attribute("seller", seller)
        .add_attribute("nft_contract", nft_contract)
        .add_attribute("token_id", token_id)
        .add_attribute("price", list_msg.price))
}

// ───── CW20 Receive: Buy or Offer ─────

fn execute_receive_cw20(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    cw20_msg: cw20::Cw20ReceiveMsg,
) -> Result<Response, ContractError> {
    let cw20_contract = info.sender.clone();
    let buyer = deps.api.addr_validate(&cw20_msg.sender)?;
    let amount = cw20_msg.amount;

    let hook_msg: Cw20HookMsg =
        from_json(&cw20_msg.msg).map_err(|_| ContractError::InvalidCw20Msg {})?;

    match hook_msg {
        Cw20HookMsg::BuyNft { listing_id } => {
            execute_buy_cw20(deps, env, buyer, cw20_contract, amount, listing_id)
        }
        Cw20HookMsg::MakeOffer {
            nft_contract,
            token_id,
            expires_in_blocks,
        } => execute_make_offer_cw20(
            deps,
            env,
            buyer,
            cw20_contract,
            amount,
            nft_contract,
            token_id,
            expires_in_blocks,
        ),
    }
}

// ───── Buy with native tokens ─────

fn execute_buy_native(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    listing_id: u64,
) -> Result<Response, ContractError> {
    // AUDIT FIX: Pause check at entry (before processing funds)
    let config = CONFIG.load(deps.storage)?;
    if config.paused {
        return Err(ContractError::Paused {});
    }

    // AUDIT FIX: Reject multi-denom sends (surplus would be locked)
    if info.funds.len() > 1 {
        return Err(ContractError::MultiDenomSend {});
    }

    let listing = LISTINGS
        .may_load(deps.storage, listing_id)?
        .ok_or(ContractError::ListingNotFound { id: listing_id })?;

    // AUDIT FIX: >= for inclusive expiry boundary
    if listing.expires_at > 0 && env.block.height >= listing.expires_at {
        return Err(ContractError::ListingExpired {});
    }

    // Verify not self-purchase
    if info.sender == listing.seller {
        return Err(ContractError::SelfPurchase {});
    }

    // Verify payment type is native
    let expected_denom = match &listing.payment {
        PaymentType::Native { denom } => denom.clone(),
        PaymentType::Cw20 { .. } => {
            return Err(ContractError::WrongPaymentType {
                expected: "cw20".to_string(),
            })
        }
    };

    // Verify payment amount
    let paid = info
        .funds
        .iter()
        .find(|c| c.denom == expected_denom)
        .map(|c| c.amount)
        .unwrap_or(Uint128::zero());

    // AUDIT FIX: Require exact payment (overpayment would be locked in contract)
    if paid != listing.price {
        return Err(ContractError::InsufficientPayment {
            expected: listing.price.to_string(),
            got: paid.to_string(),
        });
    }

    // Execute the sale
    execute_sale(
        deps,
        &info.sender,
        &listing,
        paid,
        &listing.payment.clone(),
    )
}

// ───── Buy with CW20 tokens ─────

fn execute_buy_cw20(
    deps: DepsMut,
    env: Env,
    buyer: Addr,
    cw20_contract: Addr,
    amount: Uint128,
    listing_id: u64,
) -> Result<Response, ContractError> {
    // AUDIT FIX: Pause check first; if paused we still need to refund — but
    // CW20 Receive came WITH funds already, so we must reject *and* refund.
    // Easiest: refund and stop. We use the response to send the refund.
    let config = CONFIG.load(deps.storage)?;
    if config.paused {
        return Ok(refund_cw20(&buyer, &cw20_contract, amount)?
            .add_attribute("action", "buy_nft_refund")
            .add_attribute("reason", "paused"));
    }

    let listing = match LISTINGS.may_load(deps.storage, listing_id)? {
        Some(l) => l,
        None => {
            return Ok(refund_cw20(&buyer, &cw20_contract, amount)?
                .add_attribute("action", "buy_nft_refund")
                .add_attribute("reason", "listing_not_found"));
        }
    };

    // AUDIT FIX: harmonize CW20 expiry semantics with native (>= = expired)
    if listing.expires_at > 0 && env.block.height >= listing.expires_at {
        return Ok(refund_cw20(&buyer, &cw20_contract, amount)?
            .add_attribute("action", "buy_nft_refund")
            .add_attribute("reason", "listing_expired"));
    }

    if buyer == listing.seller {
        return Ok(refund_cw20(&buyer, &cw20_contract, amount)?
            .add_attribute("action", "buy_nft_refund")
            .add_attribute("reason", "self_purchase"));
    }

    // Verify CW20 contract matches listing payment type
    match &listing.payment {
        PaymentType::Cw20 { contract_addr } => {
            if cw20_contract.as_str() != contract_addr {
                return Ok(refund_cw20(&buyer, &cw20_contract, amount)?
                    .add_attribute("action", "buy_nft_refund")
                    .add_attribute("reason", "wrong_cw20"));
            }
        }
        PaymentType::Native { .. } => {
            return Ok(refund_cw20(&buyer, &cw20_contract, amount)?
                .add_attribute("action", "buy_nft_refund")
                .add_attribute("reason", "native_only_listing"));
        }
    };

    // AUDIT FIX: require exact CW20 amount (overpayment refund-on-mismatch)
    if amount != listing.price {
        return Ok(refund_cw20(&buyer, &cw20_contract, amount)?
            .add_attribute("action", "buy_nft_refund")
            .add_attribute("reason", "wrong_amount"));
    }

    execute_sale(deps, &buyer, &listing, amount, &listing.payment.clone())
}

// ───── Core sale logic (shared by native + CW20 buys) ─────

fn execute_sale(
    deps: DepsMut,
    buyer: &Addr,
    listing: &Listing,
    paid: Uint128,
    payment: &PaymentType,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if config.paused {
        return Err(ContractError::Paused {});
    }

    // AUDIT FIX: Use actual paid amount (so accept_offer at the offer price works)
    let sale_price = paid;

    // Calculate fees
    let effective_fee_bps = get_effective_fee(deps.as_ref(), &config, buyer)?;
    let fee_amount = sale_price.multiply_ratio(effective_fee_bps as u128, 10000u128);
    let (treasury_amount, capa_amount) = if fee_amount.is_zero() || effective_fee_bps == 0 {
        (Uint128::zero(), Uint128::zero())
    } else {
        // Split using the *base* fee_bps as denominator so the configured
        // treasury_share_bps:capa_share ratio is preserved even after discount.
        let t = fee_amount
            .multiply_ratio(config.treasury_share_bps as u128, config.fee_bps as u128);
        let c = fee_amount.checked_sub(t).unwrap_or(Uint128::zero());
        (t, c)
    };

    // Calculate royalty
    let royalty = ROYALTIES.may_load(deps.storage, listing.nft_contract.as_str())?;
    let royalty_amount = match &royalty {
        Some(r) => sale_price.multiply_ratio(r.royalty_bps as u128, 10000u128),
        None => Uint128::zero(),
    };

    // AUDIT FIX: Explicit check that fees don't exceed sale price
    if fee_amount + royalty_amount > sale_price {
        return Err(ContractError::FeeExceedsPrice {});
    }

    // Seller receives: sale_price - fee - royalty
    let seller_amount = sale_price
        .checked_sub(fee_amount)?
        .checked_sub(royalty_amount)?;

    // Build transfer messages
    let mut messages: Vec<CosmosMsg> = vec![];

    match payment {
        PaymentType::Native { denom } => {
            if !seller_amount.is_zero() {
                messages.push(CosmosMsg::Bank(BankMsg::Send {
                    to_address: listing.seller.to_string(),
                    amount: vec![Coin {
                        denom: denom.clone(),
                        amount: seller_amount,
                    }],
                }));
            }
            if !treasury_amount.is_zero() {
                messages.push(CosmosMsg::Bank(BankMsg::Send {
                    to_address: config.treasury_addr.to_string(),
                    amount: vec![Coin {
                        denom: denom.clone(),
                        amount: treasury_amount,
                    }],
                }));
            }
            if !capa_amount.is_zero() {
                messages.push(CosmosMsg::Bank(BankMsg::Send {
                    to_address: config.capa_reward_addr.to_string(),
                    amount: vec![Coin {
                        denom: denom.clone(),
                        amount: capa_amount,
                    }],
                }));
            }
            if let Some(r) = &royalty {
                if !royalty_amount.is_zero() {
                    messages.push(CosmosMsg::Bank(BankMsg::Send {
                        to_address: r.recipient.to_string(),
                        amount: vec![Coin {
                            denom: denom.clone(),
                            amount: royalty_amount,
                        }],
                    }));
                }
            }
        }
        PaymentType::Cw20 { contract_addr } => {
            let cw20_addr = contract_addr.clone();
            if !seller_amount.is_zero() {
                messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
                    contract_addr: cw20_addr.clone(),
                    msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
                        recipient: listing.seller.to_string(),
                        amount: seller_amount,
                    })?,
                    funds: vec![],
                }));
            }
            if !treasury_amount.is_zero() {
                messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
                    contract_addr: cw20_addr.clone(),
                    msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
                        recipient: config.treasury_addr.to_string(),
                        amount: treasury_amount,
                    })?,
                    funds: vec![],
                }));
            }
            if !capa_amount.is_zero() {
                messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
                    contract_addr: cw20_addr.clone(),
                    msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
                        recipient: config.capa_reward_addr.to_string(),
                        amount: capa_amount,
                    })?,
                    funds: vec![],
                }));
            }
            if let Some(r) = &royalty {
                if !royalty_amount.is_zero() {
                    messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
                        contract_addr: cw20_addr,
                        msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
                            recipient: r.recipient.to_string(),
                            amount: royalty_amount,
                        })?,
                        funds: vec![],
                    }));
                }
            }
        }
    }

    // Transfer NFT to buyer
    messages.push(CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: listing.nft_contract.to_string(),
        msg: to_json_binary(&cw721::Cw721ExecuteMsg::TransferNft {
            recipient: buyer.to_string(),
            token_id: listing.token_id.clone(),
        })?,
        funds: vec![],
    }));

    // Remove listing + decrement collection counter
    LISTINGS.remove(deps.storage, listing.id);
    ACTIVE_LISTING.remove(
        deps.storage,
        (listing.nft_contract.as_str(), listing.token_id.as_str()),
    );
    decrement_collection_listings(deps.storage, listing.nft_contract.as_str())?;

    Ok(Response::new()
        .add_messages(messages)
        .add_attribute("action", "buy_nft")
        .add_attribute("listing_id", listing.id.to_string())
        .add_attribute("buyer", buyer)
        .add_attribute("seller", &listing.seller)
        .add_attribute("nft_contract", &listing.nft_contract)
        .add_attribute("token_id", &listing.token_id)
        .add_attribute("price", sale_price)
        .add_attribute("fee", fee_amount)
        .add_attribute("effective_fee_bps", effective_fee_bps.to_string())
        .add_attribute("royalty", royalty_amount)
        .add_attribute("seller_receives", seller_amount))
}

// ───── Cancel Listing ─────

fn execute_cancel_listing(
    deps: DepsMut,
    info: MessageInfo,
    listing_id: u64,
) -> Result<Response, ContractError> {
    let listing = LISTINGS
        .may_load(deps.storage, listing_id)?
        .ok_or(ContractError::ListingNotFound { id: listing_id })?;

    // Only seller or contract owner can cancel
    let config = CONFIG.load(deps.storage)?;
    let cancelled_by = if info.sender == listing.seller {
        "seller"
    } else if info.sender == config.owner {
        "admin"
    } else {
        return Err(ContractError::NotSeller {});
    };

    // Return NFT to seller
    let transfer_msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: listing.nft_contract.to_string(),
        msg: to_json_binary(&cw721::Cw721ExecuteMsg::TransferNft {
            recipient: listing.seller.to_string(),
            token_id: listing.token_id.clone(),
        })?,
        funds: vec![],
    });

    // Remove listing + decrement counter
    LISTINGS.remove(deps.storage, listing_id);
    ACTIVE_LISTING.remove(
        deps.storage,
        (listing.nft_contract.as_str(), listing.token_id.as_str()),
    );
    decrement_collection_listings(deps.storage, listing.nft_contract.as_str())?;

    Ok(Response::new()
        .add_message(transfer_msg)
        .add_attribute("action", "cancel_listing")
        .add_attribute("listing_id", listing_id.to_string())
        .add_attribute("seller", listing.seller)
        .add_attribute("cancelled_by", cancelled_by))
}

// ───── Make Offer (native) ─────

fn execute_make_offer_native(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    nft_contract: String,
    token_id: String,
    expires_in_blocks: u64,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if config.paused {
        return Err(ContractError::Paused {});
    }

    // AUDIT FIX: Reject multi-denom sends
    if info.funds.len() > 1 {
        return Err(ContractError::MultiDenomSend {});
    }

    if info.funds.is_empty() || info.funds[0].amount.is_zero() {
        return Err(ContractError::ZeroPrice {});
    }

    let coin = &info.funds[0];
    let nft_addr = deps.api.addr_validate(&nft_contract)?;

    // ─── Offer-cap gate (anti-DoS — caps refund-risk per NFT) ───
    let caps = LAUNCH_CAPS.load(deps.storage)?;
    if caps.max_active_offers_per_nft > 0 {
        let current = ACTIVE_OFFERS_PER_NFT
            .may_load(deps.storage, (nft_addr.as_str(), &token_id))?
            .unwrap_or(0);
        if current >= caps.max_active_offers_per_nft {
            return Err(ContractError::OfferCapExceeded {
                cap: caps.max_active_offers_per_nft,
            });
        }
    }

    let id = OFFER_COUNT.load(deps.storage)? + 1;
    OFFER_COUNT.save(deps.storage, &id)?;

    // AUDIT FIX: 0 = never expires (consistent with listings)
    let expires_at = if expires_in_blocks > 0 {
        env.block.height + expires_in_blocks
    } else {
        0
    };

    let offer = Offer {
        id,
        buyer: info.sender.clone(),
        nft_contract: nft_addr.clone(),
        token_id: token_id.clone(),
        price: coin.amount,
        payment: PaymentType::Native {
            denom: coin.denom.clone(),
        },
        expires_at,
        created_at: env.block.height,
    };

    OFFERS.save(deps.storage, id, &offer)?;
    OFFERS_BY_NFT.save(deps.storage, (nft_addr.as_str(), &token_id, id), &())?;
    bump_offers_per_nft(deps.storage, nft_addr.as_str(), &token_id)?;

    Ok(Response::new()
        .add_attribute("action", "make_offer")
        .add_attribute("offer_id", id.to_string())
        .add_attribute("buyer", info.sender)
        .add_attribute("nft_contract", nft_contract)
        .add_attribute("token_id", token_id)
        .add_attribute("price", coin.amount))
}

// ───── Make Offer (CW20) ─────

fn execute_make_offer_cw20(
    deps: DepsMut,
    env: Env,
    buyer: Addr,
    cw20_contract: Addr,
    amount: Uint128,
    nft_contract: String,
    token_id: String,
    expires_in_blocks: u64,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if config.paused {
        // Refund — funds were already sent
        return Ok(refund_cw20(&buyer, &cw20_contract, amount)?
            .add_attribute("action", "make_offer_refund")
            .add_attribute("reason", "paused"));
    }

    if amount.is_zero() {
        return Err(ContractError::ZeroPrice {});
    }

    let nft_addr = deps.api.addr_validate(&nft_contract)?;

    // ─── Offer-cap gate ───
    let caps = LAUNCH_CAPS.load(deps.storage)?;
    if caps.max_active_offers_per_nft > 0 {
        let current = ACTIVE_OFFERS_PER_NFT
            .may_load(deps.storage, (nft_addr.as_str(), &token_id))?
            .unwrap_or(0);
        if current >= caps.max_active_offers_per_nft {
            return Ok(refund_cw20(&buyer, &cw20_contract, amount)?
                .add_attribute("action", "make_offer_refund")
                .add_attribute("reason", "offer_cap_exceeded"));
        }
    }

    let id = OFFER_COUNT.load(deps.storage)? + 1;
    OFFER_COUNT.save(deps.storage, &id)?;

    // AUDIT FIX: 0 = never expires (consistent with native + listings)
    let expires_at = if expires_in_blocks > 0 {
        env.block.height + expires_in_blocks
    } else {
        0
    };

    let offer = Offer {
        id,
        buyer: buyer.clone(),
        nft_contract: nft_addr.clone(),
        token_id: token_id.clone(),
        price: amount,
        payment: PaymentType::Cw20 {
            contract_addr: cw20_contract.to_string(),
        },
        expires_at,
        created_at: env.block.height,
    };

    OFFERS.save(deps.storage, id, &offer)?;
    OFFERS_BY_NFT.save(deps.storage, (nft_addr.as_str(), &token_id, id), &())?;
    bump_offers_per_nft(deps.storage, nft_addr.as_str(), &token_id)?;

    Ok(Response::new()
        .add_attribute("action", "make_offer_cw20")
        .add_attribute("offer_id", id.to_string())
        .add_attribute("buyer", buyer)
        .add_attribute("amount", amount)
        .add_attribute("cw20", cw20_contract))
}

// ───── Accept Offer ─────

fn execute_accept_offer(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    offer_id: u64,
) -> Result<Response, ContractError> {
    let offer = OFFERS
        .may_load(deps.storage, offer_id)?
        .ok_or(ContractError::OfferNotFound { id: offer_id })?;

    // AUDIT FIX: >= for inclusive expiry boundary (matches listings)
    if offer.expires_at > 0 && env.block.height >= offer.expires_at {
        return Err(ContractError::OfferExpired {});
    }

    // Verify the NFT is currently listed and owned by caller
    let listing_id = ACTIVE_LISTING
        .may_load(
            deps.storage,
            (offer.nft_contract.as_str(), offer.token_id.as_str()),
        )?
        .ok_or(ContractError::NoActiveListing {})?;

    let listing = LISTINGS.load(deps.storage, listing_id)?;

    if info.sender != listing.seller {
        return Err(ContractError::NotSeller {});
    }

    // AUDIT FIX: Validate offer payment type matches listing payment type
    match (&listing.payment, &offer.payment) {
        (PaymentType::Native { denom: d1 }, PaymentType::Native { denom: d2 }) if d1 == d2 => {}
        (PaymentType::Cw20 { contract_addr: c1 }, PaymentType::Cw20 { contract_addr: c2 })
            if c1 == c2 => {}
        _ => {
            return Err(ContractError::WrongPaymentType {
                expected: format!("{:?}", listing.payment),
            })
        }
    }

    // AUDIT FIX: Remove offer BEFORE execute_sale consumes deps
    OFFERS.remove(deps.storage, offer_id);
    OFFERS_BY_NFT.remove(
        deps.storage,
        (offer.nft_contract.as_str(), offer.token_id.as_str(), offer_id),
    );
    decrement_offers_per_nft(deps.storage, offer.nft_contract.as_str(), offer.token_id.as_str())?;

    // Execute the sale using offer's payment and price
    let result = execute_sale(deps, &offer.buyer, &listing, offer.price, &offer.payment)?;

    Ok(result.add_attribute("accepted_offer_id", offer_id.to_string()))
}

// ───── Cancel Offer ─────

fn execute_cancel_offer(
    deps: DepsMut,
    info: MessageInfo,
    offer_id: u64,
) -> Result<Response, ContractError> {
    let offer = OFFERS
        .may_load(deps.storage, offer_id)?
        .ok_or(ContractError::OfferNotFound { id: offer_id })?;

    if info.sender != offer.buyer {
        return Err(ContractError::NotBuyer {});
    }

    // Refund the buyer
    let refund_msg = build_refund_msg(&offer)?;

    // Remove offer
    OFFERS.remove(deps.storage, offer_id);
    OFFERS_BY_NFT.remove(
        deps.storage,
        (offer.nft_contract.as_str(), offer.token_id.as_str(), offer_id),
    );
    decrement_offers_per_nft(deps.storage, offer.nft_contract.as_str(), offer.token_id.as_str())?;

    Ok(Response::new()
        .add_message(refund_msg)
        .add_attribute("action", "cancel_offer")
        .add_attribute("offer_id", offer_id.to_string())
        .add_attribute("buyer", offer.buyer)
        .add_attribute("refunded", offer.price))
}

// ───── Withdraw expired offer ─────

fn execute_withdraw_expired(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    offer_id: u64,
) -> Result<Response, ContractError> {
    let offer = OFFERS
        .may_load(deps.storage, offer_id)?
        .ok_or(ContractError::OfferNotFound { id: offer_id })?;

    // AUDIT FIX: never-expiring offers (expires_at == 0) cannot be withdrawn
    // by strangers — they require CancelOffer by the buyer.
    if offer.expires_at == 0 {
        return Err(ContractError::OfferNotExpired {});
    }
    // AUDIT FIX: < for inclusive expiry (matches buy_nft and accept_offer)
    if env.block.height < offer.expires_at {
        return Err(ContractError::OfferNotExpired {});
    }

    let refund_msg = build_refund_msg(&offer)?;

    OFFERS.remove(deps.storage, offer_id);
    OFFERS_BY_NFT.remove(
        deps.storage,
        (offer.nft_contract.as_str(), offer.token_id.as_str(), offer_id),
    );
    decrement_offers_per_nft(deps.storage, offer.nft_contract.as_str(), offer.token_id.as_str())?;

    Ok(Response::new()
        .add_message(refund_msg)
        .add_attribute("action", "withdraw_expired_offer")
        .add_attribute("offer_id", offer_id.to_string())
        .add_attribute("refunded_to", offer.buyer))
}

// ───── Set Royalty ─────

fn execute_set_royalty(
    deps: DepsMut,
    info: MessageInfo,
    nft_contract: String,
    recipient: String,
    royalty_bps: u16,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;

    if info.sender != config.owner {
        return Err(ContractError::NotAdmin {});
    }

    if royalty_bps > MAX_ROYALTY_BPS {
        return Err(ContractError::RoyaltyTooHigh { bps: royalty_bps });
    }

    let nft_addr = deps.api.addr_validate(&nft_contract)?;
    let recipient_addr = deps.api.addr_validate(&recipient)?;

    let royalty = RoyaltyInfo {
        recipient: recipient_addr,
        royalty_bps,
    };

    ROYALTIES.save(deps.storage, nft_addr.as_str(), &royalty)?;

    Ok(Response::new()
        .add_attribute("action", "set_royalty")
        .add_attribute("nft_contract", nft_contract)
        .add_attribute("recipient", recipient)
        .add_attribute("royalty_bps", royalty_bps.to_string()))
}

// ───── Update Config ─────

fn execute_update_config(
    deps: DepsMut,
    info: MessageInfo,
    fee_bps: Option<u16>,
    treasury_addr: Option<String>,
    capa_reward_addr: Option<String>,
    treasury_share_bps: Option<u16>,
    capa_gov_contract: Option<String>,
    paused: Option<bool>,
) -> Result<Response, ContractError> {
    let mut config = CONFIG.load(deps.storage)?;

    if info.sender != config.owner {
        return Err(ContractError::NotAdmin {});
    }

    if let Some(bps) = fee_bps {
        if bps > MAX_FEE_BPS {
            return Err(ContractError::FeeTooHigh { bps });
        }
        config.fee_bps = bps;
    }
    if let Some(addr) = treasury_addr {
        config.treasury_addr = deps.api.addr_validate(&addr)?;
    }
    if let Some(addr) = capa_reward_addr {
        config.capa_reward_addr = deps.api.addr_validate(&addr)?;
    }
    if let Some(bps) = treasury_share_bps {
        config.treasury_share_bps = bps;
    }
    if let Some(addr) = capa_gov_contract {
        config.capa_gov_contract = Some(deps.api.addr_validate(&addr)?);
    }
    if let Some(p) = paused {
        config.paused = p;
    }

    // AUDIT FIX: Re-check invariant after partial update
    if config.treasury_share_bps > config.fee_bps {
        return Err(ContractError::TreasuryShareTooHigh {});
    }

    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new().add_attribute("action", "update_config"))
}

// ───── Add / Remove allowlisted collection ─────

fn execute_add_collection(
    deps: DepsMut,
    info: MessageInfo,
    nft_contract: String,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner {
        return Err(ContractError::NotAdmin {});
    }
    let addr = deps.api.addr_validate(&nft_contract)?;
    ALLOWED_COLLECTIONS.save(deps.storage, addr.as_str(), &())?;
    Ok(Response::new()
        .add_attribute("action", "add_collection")
        .add_attribute("nft_contract", addr))
}

fn execute_remove_collection(
    deps: DepsMut,
    info: MessageInfo,
    nft_contract: String,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner {
        return Err(ContractError::NotAdmin {});
    }
    let addr = deps.api.addr_validate(&nft_contract)?;
    ALLOWED_COLLECTIONS.remove(deps.storage, addr.as_str());
    // NOTE: existing listings stay live so sellers can still cancel/sell.
    // New listings on this collection are blocked by the allowlist check.
    Ok(Response::new()
        .add_attribute("action", "remove_collection")
        .add_attribute("nft_contract", addr))
}

fn execute_set_crystal_contract(
    deps: DepsMut,
    info: MessageInfo,
    nft_contract: String,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner {
        return Err(ContractError::NotAdmin {});
    }
    let addr = deps.api.addr_validate(&nft_contract)?;
    CRYSTAL_NFT_CONTRACT.save(deps.storage, &addr)?;
    Ok(Response::new()
        .add_attribute("action", "set_crystal_contract")
        .add_attribute("nft_contract", addr))
}

fn execute_update_launch_caps(
    deps: DepsMut,
    info: MessageInfo,
    caps: LaunchCaps,
) -> Result<Response, ContractError> {
    let config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner {
        return Err(ContractError::NotAdmin {});
    }
    LAUNCH_CAPS.save(deps.storage, &caps)?;
    Ok(Response::new()
        .add_attribute("action", "update_launch_caps")
        .add_attribute("max_listings_per_collection", caps.max_active_listings_per_collection.to_string())
        .add_attribute("max_offers_per_nft", caps.max_active_offers_per_nft.to_string()))
}

fn execute_transfer_ownership(
    deps: DepsMut,
    info: MessageInfo,
    new_owner: String,
) -> Result<Response, ContractError> {
    let mut config = CONFIG.load(deps.storage)?;
    if info.sender != config.owner {
        return Err(ContractError::NotAdmin {});
    }
    let new_owner_addr = deps.api.addr_validate(&new_owner)?;
    let old = config.owner.clone();
    config.owner = new_owner_addr.clone();
    CONFIG.save(deps.storage, &config)?;
    Ok(Response::new()
        .add_attribute("action", "transfer_ownership")
        .add_attribute("old_owner", old)
        .add_attribute("new_owner", new_owner_addr))
}

// ═══════════════════════════════════════════
// HELPERS
// ═══════════════════════════════════════════

fn build_refund_msg(offer: &Offer) -> StdResult<CosmosMsg> {
    match &offer.payment {
        PaymentType::Native { denom } => Ok(CosmosMsg::Bank(BankMsg::Send {
            to_address: offer.buyer.to_string(),
            amount: vec![Coin {
                denom: denom.clone(),
                amount: offer.price,
            }],
        })),
        PaymentType::Cw20 { contract_addr } => Ok(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: contract_addr.clone(),
            msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
                recipient: offer.buyer.to_string(),
                amount: offer.price,
            })?,
            funds: vec![],
        })),
    }
}

/// Build a refund Response that immediately returns CW20 funds to the sender.
/// Used when CW20 Receive can't proceed (paused, listing not found, etc.) —
/// always refund rather than locking the user's tokens.
fn refund_cw20(buyer: &Addr, cw20: &Addr, amount: Uint128) -> StdResult<Response> {
    let msg = CosmosMsg::Wasm(WasmMsg::Execute {
        contract_addr: cw20.to_string(),
        msg: to_json_binary(&Cw20ExecuteMsg::Transfer {
            recipient: buyer.to_string(),
            amount,
        })?,
        funds: vec![],
    });
    Ok(Response::new().add_message(msg))
}

fn decrement_collection_listings(
    storage: &mut dyn cosmwasm_std::Storage,
    collection: &str,
) -> StdResult<()> {
    let current = ACTIVE_LISTINGS_PER_COLLECTION
        .may_load(storage, collection)?
        .unwrap_or(0);
    let new_count = current.saturating_sub(1);
    if new_count == 0 {
        ACTIVE_LISTINGS_PER_COLLECTION.remove(storage, collection);
    } else {
        ACTIVE_LISTINGS_PER_COLLECTION.save(storage, collection, &new_count)?;
    }
    Ok(())
}

fn bump_offers_per_nft(
    storage: &mut dyn cosmwasm_std::Storage,
    collection: &str,
    token_id: &str,
) -> StdResult<()> {
    let current = ACTIVE_OFFERS_PER_NFT
        .may_load(storage, (collection, token_id))?
        .unwrap_or(0);
    ACTIVE_OFFERS_PER_NFT.save(storage, (collection, token_id), &current.saturating_add(1))?;
    Ok(())
}

fn decrement_offers_per_nft(
    storage: &mut dyn cosmwasm_std::Storage,
    collection: &str,
    token_id: &str,
) -> StdResult<()> {
    let current = ACTIVE_OFFERS_PER_NFT
        .may_load(storage, (collection, token_id))?
        .unwrap_or(0);
    let new_count = current.saturating_sub(1);
    if new_count == 0 {
        ACTIVE_OFFERS_PER_NFT.remove(storage, (collection, token_id));
    } else {
        ACTIVE_OFFERS_PER_NFT.save(storage, (collection, token_id), &new_count)?;
    }
    Ok(())
}

/// Calculate effective fee bps for a buyer.
///
/// V1.2.0 (Cosmic-only model — Daniel 2026-04-26):
///
///   Cosmic Crystal → 0 bps   (free trading; top-tier perk, ~50 wallets)
///   Everyone else  → fee_bps (default 500 / 5.00%)
///
/// Replaces V1.1.0's 5-rung ladder. Rationale: a tiered ladder dilutes the
/// Cosmic premium and gives small discounts that nobody really values
/// (operator-feedback 2026-04-26). A single sharp cliff between Cosmic
/// (true free trading) and everyone-else (5%) makes Cosmic genuinely the
/// apex perk and keeps marketplace economics intact.
///
/// Tier resolution still walks ALTAR → FUSION → MINT contracts (mirrors
/// `feedback_crystal_tier_resolution.md` — ascended/fused crystals
/// aren't in MINT). Up to TIER_QUERY_LIMIT crystals checked per buyer
/// to bound gas. Cosmic short-circuits the loop since it's the top.
///
/// `highest_crystal_tier` still returns the highest owned tier name so
/// FeeInfoResponse can surface it for UI badges, but only "cosmic" gets
/// the discount.
fn get_effective_fee(deps: Deps, config: &Config, buyer: &Addr) -> StdResult<u16> {
    // Cosmic-only discount
    let highest = highest_crystal_tier(deps, buyer)?;
    if matches!(highest.as_deref(), Some("cosmic")) {
        return Ok(0);
    }

    // CAPA-staker discount (TODO: integrate when capa_gov_contract is set)
    if let Some(_gov) = &config.capa_gov_contract {
        // Future: query staked CAPA, walk FEE_DISCOUNT_TIERS
        // let staked: StakerResponse = deps.querier.query_wasm_smart(gov, ...)?;
    }

    Ok(config.fee_bps)
}

// ─── Crystal tier resolution chain (mainnet phoenix-1, hardcoded) ──────────
// These contract addresses are hardcoded because they're locked at the
// protocol level on phoenix-1. Future testnet deploys would rebuild with
// different consts. Reasoning: zero migration surface, no admin can mis-set,
// less attack surface. See plan_atrium_nft_marketplace.md.
const ALTAR_NFT_CONTRACT: &str   = "terra1hpjtm8q5r245a797zg0rq42wl4uk0wlvs9a6xet04jka2yj5486sjr96s4";
const FUSION_NFT_CONTRACT: &str  = "terra1hqhhw5ndpdty4yzteud20a073q93cyupv8rp3wufrv5f7d7xrc8qsuf6kg";
const MINT_NFT_CONTRACT: &str    = "terra1jez9g0kqq7lze8ncqunvxzs9d2tu4m9p5vep3d2dgs4dxh5nfj9sm9vgl4";

/// Cap on Crystals scanned per buyer to bound gas. Edge case: a whale
/// holding Cosmic at token_id > 30 with 30+ lower-id tokens may miss
/// the discount. cw721-base orders by token_id ascending, and Crystals
/// 1..50 ARE the original Cosmics, so this is a low-prevalence corner.
const TIER_QUERY_LIMIT: u32 = 30;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, schemars::JsonSchema)]
struct CrystalInfoQuery {
    crystal_info: CrystalInfoArgs,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, schemars::JsonSchema)]
struct CrystalInfoArgs {
    token_id: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, schemars::JsonSchema)]
struct CrystalInfoResponse {
    #[serde(default)]
    tier: Option<String>,
}

/// Returns the buyer's HIGHEST Crystal tier across up to TIER_QUERY_LIMIT
/// owned tokens. Returns None if buyer has no Crystals OR if every owned
/// crystal's tier resolves to None across all three sources.
///
/// In test environments where ALTAR/FUSION/MINT aren't deployed, every
/// resolve_tier() returns None and this function returns None — buyers
/// fall through to fee_bps. Documented in integration_tests.rs.
pub(crate) fn highest_crystal_tier(deps: Deps, buyer: &Addr) -> StdResult<Option<String>> {
    let crystal = CRYSTAL_NFT_CONTRACT.load(deps.storage)?;
    let resp: TokensResponse = deps.querier.query_wasm_smart(
        crystal,
        &cw721::Cw721QueryMsg::Tokens {
            owner: buyer.to_string(),
            start_after: None,
            limit: Some(TIER_QUERY_LIMIT),
        },
    )?;
    if resp.tokens.is_empty() {
        return Ok(None);
    }

    let altar  = deps.api.addr_validate(ALTAR_NFT_CONTRACT)?;
    let fusion = deps.api.addr_validate(FUSION_NFT_CONTRACT)?;
    let mint   = deps.api.addr_validate(MINT_NFT_CONTRACT)?;

    let mut highest_rank: u8 = 0;
    let mut highest_name: Option<&str> = None;

    for token_id in resp.tokens.iter() {
        let tier = resolve_tier(deps, &altar, &fusion, &mint, token_id);
        if let Some(t) = tier {
            let r = tier_rank(&t);
            if r > highest_rank {
                highest_rank = r;
                highest_name = tier_label(r);
                if r == 5 { break; } // cosmic — short-circuit
            }
        }
    }
    Ok(highest_name.map(|s| s.to_string()))
}

fn resolve_tier(deps: Deps, altar: &Addr, fusion: &Addr, mint: &Addr, token_id: &str) -> Option<String> {
    let q = CrystalInfoQuery {
        crystal_info: CrystalInfoArgs { token_id: token_id.to_string() },
    };
    if let Ok(r) = deps.querier.query_wasm_smart::<CrystalInfoResponse>(altar.clone(), &q) {
        if r.tier.is_some() { return r.tier; }
    }
    if let Ok(r) = deps.querier.query_wasm_smart::<CrystalInfoResponse>(fusion.clone(), &q) {
        if r.tier.is_some() { return r.tier; }
    }
    if let Ok(r) = deps.querier.query_wasm_smart::<CrystalInfoResponse>(mint.clone(), &q) {
        return r.tier;
    }
    None
}

fn tier_rank(t: &str) -> u8 {
    match t {
        "cosmic"    => 5,
        "prismatic" => 4,
        "radiant"   => 3,
        "charged"   => 2,
        "raw"       => 1,
        _           => 0,
    }
}

fn tier_label(rank: u8) -> Option<&'static str> {
    match rank {
        5 => Some("cosmic"),
        4 => Some("prismatic"),
        3 => Some("radiant"),
        2 => Some("charged"),
        1 => Some("raw"),
        _ => None,
    }
}

// ═══════════════════════════════════════════
// QUERY
// ═══════════════════════════════════════════

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_json_binary(&CONFIG.load(deps.storage)?),
        QueryMsg::Listing { listing_id } => to_json_binary(&LISTINGS.load(deps.storage, listing_id)?),
        QueryMsg::ListingsByCollection {
            nft_contract,
            start_after,
            limit,
        } => to_json_binary(&query_listings_by_collection(
            deps,
            nft_contract,
            start_after,
            limit,
        )?),
        QueryMsg::ListingsBySeller {
            seller,
            start_after,
            limit,
        } => to_json_binary(&query_listings_by_seller(deps, seller, start_after, limit)?),
        QueryMsg::AllListings { start_after, limit } => {
            to_json_binary(&query_all_listings(deps, start_after, limit)?)
        }
        QueryMsg::Offer { offer_id } => to_json_binary(&OFFERS.load(deps.storage, offer_id)?),
        QueryMsg::OffersByNft {
            nft_contract,
            token_id,
            start_after,
            limit,
        } => to_json_binary(&query_offers_by_nft(
            deps,
            nft_contract,
            token_id,
            start_after,
            limit,
        )?),
        QueryMsg::Royalty { nft_contract } => {
            let royalty = ROYALTIES.may_load(deps.storage, &nft_contract)?;
            to_json_binary(&RoyaltyInfoResponse { royalty })
        }
        QueryMsg::FeeInfo { buyer } => {
            let config = CONFIG.load(deps.storage)?;
            let (fee_bps, discount_bps, tier_opt) = if let Some(b) = buyer {
                let addr = deps.api.addr_validate(&b)?;
                let eff = get_effective_fee(deps, &config, &addr)?;
                let t = highest_crystal_tier(deps, &addr).unwrap_or(None);
                // saturating_sub guards a defensive underflow path
                (eff, config.fee_bps.saturating_sub(eff), t)
            } else {
                (config.fee_bps, 0, None)
            };
            // crystal_holder kept for backwards compat — derived from tier
            let holder = tier_opt.is_some();
            to_json_binary(&FeeInfoResponse {
                fee_bps,
                capa_staked: Uint128::zero(),
                discount_bps,
                crystal_holder: holder,
                crystal_tier: tier_opt,
            })
        }
        QueryMsg::IsCollectionAllowed { nft_contract } => {
            let allowed = ALLOWED_COLLECTIONS.has(deps.storage, &nft_contract);
            to_json_binary(&IsAllowedResponse { allowed })
        }
        QueryMsg::AllowedCollections { start_after, limit } => {
            to_json_binary(&query_allowed_collections(deps, start_after, limit)?)
        }
        QueryMsg::CollectionStats { nft_contract } => {
            let caps = LAUNCH_CAPS.load(deps.storage)?;
            let count = ACTIVE_LISTINGS_PER_COLLECTION
                .may_load(deps.storage, &nft_contract)?
                .unwrap_or(0);
            let allowed = ALLOWED_COLLECTIONS.has(deps.storage, &nft_contract);
            to_json_binary(&CollectionStatsResponse {
                nft_contract,
                active_listings: count,
                cap: caps.max_active_listings_per_collection,
                allowed,
            })
        }
        QueryMsg::LaunchCaps {} => to_json_binary(&LAUNCH_CAPS.load(deps.storage)?),
    }
}

fn query_all_listings(
    deps: Deps,
    start_after: Option<u64>,
    limit: Option<u32>,
) -> StdResult<ListingsResponse> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
    let start = start_after.map(cw_storage_plus::Bound::exclusive);

    let listings: Vec<Listing> = LISTINGS
        .range(deps.storage, start, None, Order::Ascending)
        .take(limit)
        .map(|item| item.map(|(_, l)| l))
        .collect::<StdResult<Vec<_>>>()?;

    Ok(ListingsResponse { listings })
}

fn query_listings_by_collection(
    deps: Deps,
    nft_contract: String,
    start_after: Option<u64>,
    limit: Option<u32>,
) -> StdResult<ListingsResponse> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
    let start = start_after.map(cw_storage_plus::Bound::exclusive);

    // V1: linear scan (acceptable while caps keep totals bounded).
    // V2 candidate: secondary IndexedMap by nft_contract.
    let listings: Vec<Listing> = LISTINGS
        .range(deps.storage, start, None, Order::Ascending)
        .filter_map(|item| {
            item.ok().and_then(|(_, l)| {
                if l.nft_contract.as_str() == nft_contract {
                    Some(l)
                } else {
                    None
                }
            })
        })
        .take(limit)
        .collect();

    Ok(ListingsResponse { listings })
}

fn query_listings_by_seller(
    deps: Deps,
    seller: String,
    start_after: Option<u64>,
    limit: Option<u32>,
) -> StdResult<ListingsResponse> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
    let start = start_after.map(cw_storage_plus::Bound::exclusive);

    let listings: Vec<Listing> = LISTINGS
        .range(deps.storage, start, None, Order::Ascending)
        .filter_map(|item| {
            item.ok().and_then(|(_, l)| {
                if l.seller.as_str() == seller {
                    Some(l)
                } else {
                    None
                }
            })
        })
        .take(limit)
        .collect();

    Ok(ListingsResponse { listings })
}

fn query_offers_by_nft(
    deps: Deps,
    nft_contract: String,
    token_id: String,
    start_after: Option<u64>,
    limit: Option<u32>,
) -> StdResult<OffersResponse> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
    let start = start_after.map(cw_storage_plus::Bound::exclusive);

    let offers: Vec<Offer> = OFFERS_BY_NFT
        .prefix((nft_contract.as_str(), token_id.as_str()))
        .range(deps.storage, start, None, Order::Ascending)
        .take(limit)
        .filter_map(|item| {
            item.ok()
                .and_then(|(offer_id, _)| OFFERS.load(deps.storage, offer_id).ok())
        })
        .collect();

    Ok(OffersResponse { offers })
}

fn query_allowed_collections(
    deps: Deps,
    start_after: Option<String>,
    limit: Option<u32>,
) -> StdResult<AllowedCollectionsResponse> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
    let start = start_after.as_deref().map(cw_storage_plus::Bound::exclusive);

    let collections: Vec<String> = ALLOWED_COLLECTIONS
        .range(deps.storage, start, None, Order::Ascending)
        .take(limit)
        .filter_map(|item| item.ok().map(|(k, _)| k))
        .collect();

    Ok(AllowedCollectionsResponse { collections })
}

// ═══════════════════════════════════════════
// SUPPRESS UNUSED
// ═══════════════════════════════════════════

#[allow(dead_code)]
const _: [(u128, u16); 4] = FEE_DISCOUNT_TIERS;
