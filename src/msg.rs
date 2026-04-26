use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::Uint128;
use crate::state::{
    CollectionOffer, Config, LaunchCaps, Listing, Offer, PaymentType, RoyaltyInfo,
    TraitConstraint, TraitRegistry,
};

// ═══════════════════════════════════════════
// INSTANTIATE
// ═══════════════════════════════════════════

// V1.1.0 migration takes no parameters — the contract just bumps its
// cw2 version marker and starts using the new tier-ladder fee logic.
#[cw_serde]
pub struct MigrateMsg {}

#[cw_serde]
pub struct InstantiateMsg {
    /// Total marketplace fee in basis points (e.g. 150 = 1.5%)
    pub fee_bps: u16,
    /// Treasury address
    pub treasury_addr: String,
    /// CAPA staking reward pool address
    pub capa_reward_addr: String,
    /// How much of the fee goes to treasury (bps out of fee_bps)
    /// e.g. 100 means treasury gets 100 bps (1.0%) and CAPA pool gets remainder
    pub treasury_share_bps: u16,
    /// Optional: CAPA governance staking contract for fee discounts
    pub capa_gov_contract: Option<String>,
    /// CAPA Crystal CW721 contract — holders pay 0% marketplace fee.
    pub crystal_nft_contract: String,
    /// Initial allowlisted CW721 collections (V1 typically just [Crystal]).
    pub initial_collections: Vec<String>,
    /// Launch caps (anti-DoS rails for the first 30 days).
    pub launch_caps: LaunchCaps,
}

// ═══════════════════════════════════════════
// EXECUTE
// ═══════════════════════════════════════════

#[cw_serde]
pub enum ExecuteMsg {
    /// Receive CW721 NFT → creates a listing
    /// Triggered by cw721::SendNft with msg containing ListNftMsg
    ReceiveNft(cw721::Cw721ReceiveMsg),

    /// Receive CW20 tokens → executes a buy or creates an offer
    /// Triggered by cw20::Send with msg containing Cw20HookMsg
    Receive(cw20::Cw20ReceiveMsg),

    /// Buy a listed NFT with native tokens
    BuyNft {
        listing_id: u64,
    },

    /// Cancel your own listing, get NFT back
    CancelListing {
        listing_id: u64,
    },

    /// Make an offer on an NFT with native tokens
    MakeOffer {
        nft_contract: String,
        token_id: String,
        expires_in_blocks: u64,
    },

    /// Accept an offer on your listed NFT
    AcceptOffer {
        offer_id: u64,
    },

    /// Cancel your own offer, get funds back
    CancelOffer {
        offer_id: u64,
    },

    /// Withdraw an expired offer (anyone can call — funds go to original buyer)
    WithdrawExpiredOffer {
        offer_id: u64,
    },

    /// Set royalty info for a collection (admin only in V1)
    SetRoyalty {
        nft_contract: String,
        recipient: String,
        royalty_bps: u16,
    },

    /// Update contract config (admin only)
    UpdateConfig {
        fee_bps: Option<u16>,
        treasury_addr: Option<String>,
        capa_reward_addr: Option<String>,
        treasury_share_bps: Option<u16>,
        capa_gov_contract: Option<String>,
        paused: Option<bool>,
    },

    // ─── Curation / launch-safety (admin only) ──────────────────────────

    /// Allowlist a CW721 collection so its tokens can be listed.
    AddCollection { nft_contract: String },

    /// Remove a collection from the allowlist (existing listings stay live).
    RemoveCollection { nft_contract: String },

    /// Update the Crystal NFT contract used for the holder-discount lookup.
    SetCrystalContract { nft_contract: String },

    /// Update the launch caps (admin can relax once we're confident).
    UpdateLaunchCaps { caps: LaunchCaps },

    /// Transfer admin ownership.
    TransferOwnership { new_owner: String },

    // ─── V1.3.0: Collection offers ──────────────────────────────────────

    /// Make a collection offer paying with native tokens. The amount sent
    /// MUST equal `price_per_nft * max_trades`.
    /// • `constraints` empty + `max_trades=1` = simple "buy any token" offer
    /// • `constraints` non-empty + `max_trades=1` = trait-filtered offer
    /// • `max_trades>1` = bulk floor-defense (drains as sellers fill)
    MakeCollectionOffer {
        nft_contract: String,
        price_per_nft: Uint128,
        constraints: Vec<TraitConstraint>,
        max_trades: u32,
        expires_in_blocks: u64,
    },

    /// Seller fulfills (one slot of) a collection offer using a token they
    /// have actively listed. Must supply a merkle proof per constraint
    /// proving the token's traits satisfy the constraint (proofs MAY be
    /// empty when constraints is empty).
    AcceptCollectionOffer {
        offer_id: u64,
        token_id: String,
        /// One proof per constraint. Each `TraitProof` proves the seller's
        /// token has a trait_type=value that matches the constraint at the
        /// same index in the offer's constraints vec.
        proofs: Vec<TraitProof>,
    },

    /// Buyer cancels their own collection offer; remaining escrow refunded.
    CancelCollectionOffer { offer_id: u64 },

    /// Anyone can withdraw an expired collection offer (refund the buyer).
    WithdrawExpiredCollectionOffer { offer_id: u64 },

    /// Set/replace the trait registry for a collection (admin only in V1.3).
    /// Pushing a new root invalidates outstanding proofs — UI should warn.
    SetTraitRegistry {
        nft_contract: String,
        merkle_root_hex: String,    // 64 hex chars = 32 bytes
        source_url: Option<String>,
    },
}

/// Merkle proof entry submitted by seller at AcceptCollectionOffer time.
/// Proves that token `(token_id from execute msg)` has trait
/// `trait_type=trait_value` per the collection's trait registry.
#[cw_serde]
pub struct TraitProof {
    /// Which trait this proof attests to (must match the constraint at the
    /// same index in offer.constraints).
    pub trait_type: String,
    /// The value the seller is claiming for that trait (must be one of
    /// the constraint.accepted_values).
    pub trait_value: String,
    /// Sibling hashes (32 bytes each) bottom-up. Hex-encoded for ergonomics.
    pub sibling_hashes_hex: Vec<String>,
    /// Per-level direction: true = sibling is on the right (we hash
    /// hash(self || sibling)), false = sibling is on the left
    /// (we hash hash(sibling || self)).
    pub sibling_on_right: Vec<bool>,
}

/// Message embedded in CW721 SendNft callback
#[cw_serde]
pub struct ListNftMsg {
    pub price: Uint128,
    pub payment: PaymentType,
    /// Blocks until expiry (0 = never expires)
    pub expires_in_blocks: u64,
    /// V1.4.0: optional. When set, ONLY this wallet can BuyNft AND only
    /// an Offer from this wallet can be AcceptedOffer'd by the seller.
    /// Use for OTC / peer-to-peer private sales. None = open listing.
    /// Defaults to None for backwards compatibility with V1.3 frontends.
    #[serde(default)]
    pub whitelisted_buyer: Option<String>,
}

/// Message embedded in CW20 Send callback
#[cw_serde]
pub enum Cw20HookMsg {
    /// Buy a listed NFT paying with CW20 tokens
    BuyNft { listing_id: u64 },
    /// Make an offer on an NFT paying with CW20 tokens
    MakeOffer {
        nft_contract: String,
        token_id: String,
        expires_in_blocks: u64,
    },
    /// V1.3: Make a collection offer paying with CW20 tokens.
    /// Cw20.amount sent MUST equal price_per_nft * max_trades.
    MakeCollectionOffer {
        nft_contract: String,
        price_per_nft: Uint128,
        constraints: Vec<TraitConstraint>,
        max_trades: u32,
        expires_in_blocks: u64,
    },
}

// ═══════════════════════════════════════════
// QUERY
// ═══════════════════════════════════════════

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(Config)]
    Config {},

    #[returns(Listing)]
    Listing { listing_id: u64 },

    #[returns(ListingsResponse)]
    ListingsByCollection {
        nft_contract: String,
        start_after: Option<u64>,
        limit: Option<u32>,
    },

    #[returns(ListingsResponse)]
    ListingsBySeller {
        seller: String,
        start_after: Option<u64>,
        limit: Option<u32>,
    },

    #[returns(ListingsResponse)]
    AllListings {
        start_after: Option<u64>,
        limit: Option<u32>,
    },

    #[returns(Offer)]
    Offer { offer_id: u64 },

    #[returns(OffersResponse)]
    OffersByNft {
        nft_contract: String,
        token_id: String,
        start_after: Option<u64>,
        limit: Option<u32>,
    },

    #[returns(RoyaltyInfoResponse)]
    Royalty { nft_contract: String },

    #[returns(FeeInfoResponse)]
    FeeInfo { buyer: Option<String> },

    /// Whether `nft_contract` is on the allowlist.
    #[returns(IsAllowedResponse)]
    IsCollectionAllowed { nft_contract: String },

    /// Paginated list of allowlisted collections.
    #[returns(AllowedCollectionsResponse)]
    AllowedCollections {
        start_after: Option<String>,
        limit: Option<u32>,
    },

    /// Active-listing count for a collection (vs. cap).
    #[returns(CollectionStatsResponse)]
    CollectionStats { nft_contract: String },

    /// Current launch caps.
    #[returns(LaunchCaps)]
    LaunchCaps {},

    // ─── V1.3.0: Collection offers + trait registry ─────────────────────

    #[returns(CollectionOffer)]
    CollectionOffer { offer_id: u64 },

    #[returns(CollectionOffersResponse)]
    CollectionOffersForCollection {
        nft_contract: String,
        start_after: Option<u64>,
        limit: Option<u32>,
    },

    #[returns(CollectionOffersResponse)]
    CollectionOffersByBuyer {
        buyer: String,
        start_after: Option<u64>,
        limit: Option<u32>,
    },

    /// Returns the trait registry (merkle root) for a collection.
    #[returns(TraitRegistryResponse)]
    TraitRegistry { nft_contract: String },
}

// ═══════════════════════════════════════════
// RESPONSES
// ═══════════════════════════════════════════

#[cw_serde]
pub struct ListingsResponse {
    pub listings: Vec<Listing>,
}

#[cw_serde]
pub struct OffersResponse {
    pub offers: Vec<Offer>,
}

#[cw_serde]
pub struct RoyaltyInfoResponse {
    pub royalty: Option<RoyaltyInfo>,
}

#[cw_serde]
pub struct FeeInfoResponse {
    /// Effective fee in bps after discount
    pub fee_bps: u16,
    /// CAPA staked by buyer (0 if no buyer specified)
    pub capa_staked: Uint128,
    /// Discount applied in bps
    pub discount_bps: u16,
    /// Whether buyer holds at least one CAPA Crystal of any tier.
    /// Kept for backwards compat — derived from `crystal_tier.is_some()`.
    pub crystal_holder: bool,
    /// V1.1.0: highest Crystal tier owned by buyer. Drives the fee ladder:
    /// cosmic→0bps, prismatic→25bps, radiant→50bps, charged→100bps,
    /// raw→fee_bps (no discount), null→no Crystals owned.
    pub crystal_tier: Option<String>,
}

#[cw_serde]
pub struct IsAllowedResponse {
    pub allowed: bool,
}

#[cw_serde]
pub struct AllowedCollectionsResponse {
    pub collections: Vec<String>,
}

#[cw_serde]
pub struct CollectionStatsResponse {
    pub nft_contract: String,
    pub active_listings: u32,
    pub cap: u32,
    pub allowed: bool,
}

// ─── V1.3.0 responses ──────────────────────────────────────────────────

#[cw_serde]
pub struct CollectionOffersResponse {
    pub offers: Vec<CollectionOffer>,
}

#[cw_serde]
pub struct TraitRegistryResponse {
    pub registry: Option<TraitRegistry>,
}
