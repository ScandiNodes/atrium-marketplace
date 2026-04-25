use cosmwasm_std::{OverflowError, StdError};
use thiserror::Error;

/// Atrium marketplace errors.
///
/// StoryBrand framing: each error tells the user (the hero) what went wrong
/// AND surfaces enough context that the recovery path is obvious. We never
/// emit a bare "Unauthorized" — we say *which* role the caller is missing.
#[derive(Error, Debug)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("{0}")]
    Overflow(#[from] OverflowError),

    #[error("Caller is not the contract admin")]
    NotAdmin {},

    #[error("Caller is not the listing's seller")]
    NotSeller {},

    #[error("Caller is not the offer's buyer")]
    NotBuyer {},

    #[error("Marketplace is paused — try again after the admin re-opens it")]
    Paused {},

    #[error("Listing not found: {id}")]
    ListingNotFound { id: u64 },

    #[error("Offer not found: {id}")]
    OfferNotFound { id: u64 },

    #[error("No active listing for this NFT")]
    NoActiveListing {},

    #[error("Listing has expired")]
    ListingExpired {},

    #[error("Offer has expired")]
    OfferExpired {},

    #[error("Offer has not expired yet")]
    OfferNotExpired {},

    #[error("NFT already listed — cancel the existing listing first")]
    AlreadyListed {},

    #[error("Insufficient payment: expected {expected}, got {got}")]
    InsufficientPayment { expected: String, got: String },

    #[error("Wrong payment type: expected {expected}")]
    WrongPaymentType { expected: String },

    #[error("Price must be greater than zero")]
    ZeroPrice {},

    #[error("Royalty too high: max 15% (1500 bps), got {bps}")]
    RoyaltyTooHigh { bps: u16 },

    #[error("Fee too high: max 5% (500 bps), got {bps}")]
    FeeTooHigh { bps: u16 },

    #[error("Cannot buy your own listing")]
    SelfPurchase {},

    #[error("Invalid CW721 receive message")]
    InvalidCw721Msg {},

    #[error("Invalid CW20 receive message")]
    InvalidCw20Msg {},

    // ─── Launch-safety errors ────────────────────────────────────────────

    /// Collection is not in the curated allowlist.
    /// V1 is admin-curated; admin can call AddCollection to allowlist it.
    #[error("Collection not allowlisted: {addr} — admin must add it before listing")]
    CollectionNotAllowed { addr: String },

    /// Active-listings cap reached for this collection (anti-DoS rail).
    #[error("Collection has reached its active-listing cap ({cap}) — wait for sales/cancels or ask admin to raise the cap")]
    ListingCapExceeded { cap: u32 },

    /// Active-offers cap reached for this NFT (caps refund-risk per token).
    #[error("This NFT has reached its active-offer cap ({cap}) — wait for one to settle or expire")]
    OfferCapExceeded { cap: u32 },

    /// Treasury share misconfigured at instantiate.
    #[error("treasury_share_bps cannot exceed fee_bps")]
    TreasuryShareTooHigh {},

    /// Royalty + fee combined exceed the sale price (would underflow seller payout).
    #[error("Fee + royalty exceeds sale price — refusing to settle a negative payout")]
    FeeExceedsPrice {},

    /// Multi-coin send to a single-denom entrypoint.
    #[error("Send exactly one coin denomination — multi-denom sends would lock the surplus")]
    MultiDenomSend {},
}
