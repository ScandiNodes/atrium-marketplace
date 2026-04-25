use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::Uint128;
use crate::state::{Config, LaunchCaps, Listing, Offer, PaymentType, RoyaltyInfo};

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
}

/// Message embedded in CW721 SendNft callback
#[cw_serde]
pub struct ListNftMsg {
    pub price: Uint128,
    pub payment: PaymentType,
    /// Blocks until expiry (0 = never expires)
    pub expires_in_blocks: u64,
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
