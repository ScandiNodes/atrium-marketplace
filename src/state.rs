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
    /// V1.4.0: optional private-listing target. When `Some(addr)`, only
    /// that wallet can BuyNft (direct buy) AND only an Offer from that
    /// wallet can be Accepted by the seller. Used for OTC deals where
    /// the seller wants to lock the listing to a known counterparty.
    /// `None` = open listing (V1.0 behaviour).
    /// `#[serde(default)]` keeps V1.3-stored listings deserialisable
    /// post-migrate (they have no whitelisted_buyer field yet → None).
    #[serde(default)]
    pub whitelisted_buyer: Option<Addr>,
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
// COLLECTION OFFER (V1.3.0+)
// ═══════════════════════════════════════════
//
// Buyer-initiated offer that targets ANY token in a collection (not a
// specific token_id). When trait-constraints are non-empty, only tokens
// matching the constraints can fulfil the offer. When max_trades > 1,
// the offer is "bulk" — multiple sellers can fulfil it (one accept per
// max_trades) until the escrow drains.
//
// Used for:
//   • Single-fill collection offer (max_trades=1, no constraints) —
//     "I'll buy any aDAO bird for X SOLID"
//   • Trait-filtered collection offer (max_trades=1, constraints set) —
//     "I'll buy any UNBROKEN aDAO bird for X SOLID"
//   • Bulk floor-defense (max_trades=N, constraints set) — "I'll buy
//     up to 100 unbroken aDAO birds for 20 SOLID each"

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct CollectionOffer {
    pub id: u64,
    pub buyer: Addr,
    pub nft_contract: Addr,
    /// Per-NFT price. Total escrow at create-time = price_per_nft * max_trades.
    pub price_per_nft: Uint128,
    pub payment: PaymentType,
    /// Trait constraints (AND across constraints, OR within each constraint).
    /// Empty = no filter (any NFT in collection matches).
    pub constraints: Vec<TraitConstraint>,
    /// How many fills this offer accepts before auto-closing. ≥1.
    pub max_trades: u32,
    /// Fills used so far. When trades_filled == max_trades, offer closes.
    pub trades_filled: u32,
    /// Remaining escrow (decrements by price_per_nft each fill).
    pub escrow_balance: Uint128,
    /// Block height when offer expires (0 = never).
    pub expires_at: u64,
    pub created_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct TraitConstraint {
    /// Trait name as recorded in the trait registry. Examples:
    /// "Status", "Tier", "Background"
    pub trait_type: String,
    /// Accepted values (OR-semantics within a constraint). Examples:
    /// ["Unbroken"], ["Cosmic", "Prismatic"]
    pub accepted_values: Vec<String>,
}

// ═══════════════════════════════════════════
// TRAIT REGISTRY (V1.3.0+)
// ═══════════════════════════════════════════
//
// Per-collection merkle root over (token_id, traits) pairs. Set by the
// admin when allowlisting a collection (or later via SetTraitRegistry).
// At AcceptCollectionOffer time, the seller submits a merkle proof for
// their token's traits which is verified against the stored root.
//
// Leaf encoding: sha256(token_id || "|" || trait_type || "=" || trait_value)
// — one leaf per trait, NOT one leaf per token. Multi-trait NFTs need
// one proof per (constraint.trait_type, accepted_value) match.
//
// Collections without a registry CAN still receive collection offers
// IF the offer has empty constraints (no filter needed → no proofs).
// Trait-aware offers REQUIRE the collection to have a trait-registry.

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct TraitRegistry {
    /// 32-byte sha256 merkle root over the leaf-set.
    pub merkle_root: [u8; 32],
    /// Block height of last update (informational).
    pub updated_at: u64,
    /// Address that pushed this root (admin or collection-admin).
    pub updated_by: Addr,
    /// Optional URL to the leaf-set JSON (off-chain availability hint).
    pub source_url: Option<String>,
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

// ─── V1.3.0: Collection offers + trait registry ─────────────────────────────

pub const COLLECTION_OFFER_COUNT: Item<u64> = Item::new("co_c");
pub const COLLECTION_OFFERS: Map<u64, CollectionOffer> = Map::new("co");

// Index: (collection_addr, offer_id) → () for cheap "all offers on collection X"
pub const COLLECTION_OFFERS_BY_COLLECTION: Map<(&str, u64), ()> = Map::new("co_bc");

// Per-collection trait registry (merkle root). Optional — only required for
// trait-aware accepts; trait-free accepts work without a registry.
pub const TRAIT_REGISTRY: Map<&str, TraitRegistry> = Map::new("tr");

/// V1.3 hard cap on simultaneous fills inside one bulk collection-offer.
/// Stops a malicious buyer from minting a 1M-trade offer that locks
/// massive escrow and sprays event-noise. Tunable via UpdateLaunchCaps
/// in V1.4 if needed.
pub const MAX_TRADES_PER_OFFER: u32 = 1000;

/// V1.3 hard cap on merkle-proof depth (= leaves up to 2^16 = 65536).
/// Bigger registries need a different verification scheme; for V1.3 we
/// assume per-collection token-set ≤ ~64K which fits Atrium scope easily.
pub const MAX_MERKLE_DEPTH: usize = 16;
