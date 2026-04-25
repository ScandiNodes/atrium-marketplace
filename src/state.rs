use cosmwasm_std::{Addr, Uint128};
use cw_storage_plus::{Item, Map};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════
// CONFIG
// ═══════════════════════════════════════════

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct Config {
    /// Contract admin
    pub owner: Addr,
    /// Total marketplace fee in basis points (150 = 1.5%)
    pub fee_bps: u16,
    /// Treasury address (receives 2/3 of fee = 1.0%)
    pub treasury_addr: Addr,
    /// CAPA reward pool address (receives 1/3 of fee = 0.5%)
    pub capa_reward_addr: Addr,
    /// Ratio of fee going to treasury (in bps out of fee_bps)
    /// e.g. 100 out of 150 = treasury gets 1.0%
    pub treasury_share_bps: u16,
    /// CAPA governance staking contract (for CAPA-staking fee discount queries)
    pub capa_gov_contract: Option<Addr>,
    /// Whether the marketplace is paused
    pub paused: bool,
}

// ═══════════════════════════════════════════
// LAUNCH CAPS — surgical safety rails
// ═══════════════════════════════════════════
//
// Hormozi value-equation framing: lower the perceived risk by capping
// blast-radius for the first 30 days. Admin can relax via UpdateLaunchCaps.

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct LaunchCaps {
    /// Max simultaneous active listings per collection (0 = unlimited).
    pub max_active_listings_per_collection: u32,
    /// Max simultaneous active offers per NFT (0 = unlimited).
    /// Limits per-NFT refund-risk if pause is hit mid-flight.
    pub max_active_offers_per_nft: u32,
}

// ═══════════════════════════════════════════
// LISTING
// ═══════════════════════════════════════════

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct Listing {
    pub id: u64,
    pub seller: Addr,
    pub nft_contract: Addr,
    pub token_id: String,
    /// Price amount in the smallest denomination
    pub price: Uint128,
    /// Payment type: native denom string OR CW20 contract address
    pub payment: PaymentType,
    /// Block height when listing expires (0 = never)
    pub expires_at: u64,
    /// Block height when listed
    pub created_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub enum PaymentType {
    /// Native token (e.g. "uluna", "ibc/2C962D...")
    Native { denom: String },
    /// CW20 token (e.g. SOLID contract address)
    Cw20 { contract_addr: String },
}

// ═══════════════════════════════════════════
// OFFER
// ═══════════════════════════════════════════

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct Offer {
    pub id: u64,
    pub buyer: Addr,
    pub nft_contract: Addr,
    pub token_id: String,
    pub price: Uint128,
    pub payment: PaymentType,
    /// Block height when offer expires (0 = never)
    pub expires_at: u64,
    pub created_at: u64,
}

// ═══════════════════════════════════════════
// ROYALTY
// ═══════════════════════════════════════════

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct RoyaltyInfo {
    /// Who receives royalties
    pub recipient: Addr,
    /// Royalty in basis points (max 1500 = 15%)
    pub royalty_bps: u16,
}

// ═══════════════════════════════════════════
// STORAGE
// ═══════════════════════════════════════════

pub const CONFIG: Item<Config> = Item::new("config");

// Auto-incrementing counters
pub const LISTING_COUNT: Item<u64> = Item::new("lc");
pub const OFFER_COUNT: Item<u64> = Item::new("oc");

// Primary storage
pub const LISTINGS: Map<u64, Listing> = Map::new("l");
pub const OFFERS: Map<u64, Offer> = Map::new("o");

// Royalties per collection
pub const ROYALTIES: Map<&str, RoyaltyInfo> = Map::new("r");

// Indexes: (nft_contract, token_id) → active listing_id
// Only one active listing per NFT at a time
pub const ACTIVE_LISTING: Map<(&str, &str), u64> = Map::new("al");

// Index: offers by NFT — (nft_contract, token_id, offer_id) → ()
pub const OFFERS_BY_NFT: Map<(&str, &str, u64), ()> = Map::new("on");

// ─── Curation / launch safety ──────────────────────────────────────────────
//
// Allowlisted CW721 contracts. Only collections in this map can be listed.
// V1 starts with CAPA Crystals only; admin adds collections one by one.
// (&str = collection contract address)
pub const ALLOWED_COLLECTIONS: Map<&str, ()> = Map::new("ac");

// CAPA Crystal CW721 contract — Crystal holders pay 0% marketplace fee.
// Single source-of-truth for the holder-discount lookup.
pub const CRYSTAL_NFT_CONTRACT: Item<Addr> = Item::new("crystal_nft");

// Per-collection active listing counter (for cap enforcement)
pub const ACTIVE_LISTINGS_PER_COLLECTION: Map<&str, u32> = Map::new("alc");

// Per-NFT active offer counter (for cap enforcement)
pub const ACTIVE_OFFERS_PER_NFT: Map<(&str, &str), u32> = Map::new("aon");

// Launch caps — admin-tunable
pub const LAUNCH_CAPS: Item<LaunchCaps> = Item::new("launch_caps");

// Fee discount tiers (CAPA staked → fee discount in bps)
// 0 CAPA = 0 discount, 1K = 25 bps, 10K = 50 bps, 50K = 75 bps
pub const FEE_DISCOUNT_TIERS: [(u128, u16); 4] = [
    (50_000_000_000, 75),  // 50K+ CAPA → 75 bps discount (0.75% fee)
    (10_000_000_000, 50),  // 10K+ CAPA → 50 bps discount (1.0% fee)
    (1_000_000_000, 25),   // 1K+ CAPA  → 25 bps discount (1.25% fee)
    (0, 0),                // 0 CAPA    → no discount (1.5% fee)
];
