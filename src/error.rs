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

    // ─── V1.3.0: Collection offers + trait registry ──────────────────────

    #[error("Collection offer not found: {id}")]
    CollectionOfferNotFound { id: u64 },

    #[error("Collection offer has expired")]
    CollectionOfferExpired {},

    #[error("Collection offer is fully filled — no remaining capacity")]
    CollectionOfferFull {},

    /// Trait constraints can only be applied to collections with a registered
    /// merkle-trait-root. Push a registry via SetTraitRegistry first.
    #[error("Collection has no trait registry — cannot apply trait constraints")]
    NoTraitRegistry {},

    /// Buyer requested too many fills (max_trades) on a single collection
    /// offer; bound exists to cap escrow/event-spam from a single buyer.
    #[error("max_trades exceeds the per-offer ceiling ({cap})")]
    MaxTradesTooHigh { cap: u32 },

    /// Buyer requested zero fills.
    #[error("max_trades must be at least 1")]
    MaxTradesZero {},

    /// Funds buyer escrowed for a bulk offer don't match price_per_nft × max_trades.
    #[error("Escrow mismatch: expected {expected} (= price_per_nft × max_trades), got {got}")]
    EscrowMismatch { expected: String, got: String },

    /// Merkle proof failed verification against the registered root.
    #[error("Merkle proof failed verification — token's traits don't match the registry root")]
    BadMerkleProof {},

    /// Provided merkle proof is too long (exceeds MAX_MERKLE_DEPTH).
    #[error("Merkle proof too deep (exceeds cap)")]
    MerkleProofTooDeep {},

    /// Buyer's submitted token does not satisfy one or more trait constraints.
    #[error("Token traits don't match the offer's constraints (need {trait_type} ∈ {accepted_values:?})")]
    TraitConstraintFailed {
        trait_type: String,
        accepted_values: Vec<String>,
    },

    /// Constraint count + proofs supplied don't line up.
    #[error("One merkle proof required per constraint — got {got}, expected {expected}")]
    ProofCountMismatch { got: usize, expected: usize },

    /// NFT for an accept-collection-offer must be currently listed AND owned
    /// by the seller-initiator (mirrors AcceptOffer's NoActiveListing rail).
    #[error("NFT must be actively listed by you to fulfil this collection offer")]
    NotListedBySeller {},

    // ─── V1.4.0: Private listings ────────────────────────────────────────

    /// Listing has whitelisted_buyer set; the calling buyer (or the offer's
    /// buyer at accept-time) is not the whitelisted address.
    #[error("This listing is private — only {whitelisted} can buy it")]
    ListingPrivate { whitelisted: String },

    // ─── V1.5.0: Vesting (TLA-Lock) + promo whitelist ────────────────────

    /// Release{} called before the unlock height was reached.
    #[error("Vesting period not over — releases at block {unlock_at} (current {current})")]
    LockNotExpired { unlock_at: u64, current: u64 },

    /// Release{} called on a listing that hasn't been bought yet, or on a
    /// non-vesting listing.
    #[error("Listing is not in vesting/locked state")]
    NotInLockedState {},

    /// Cancel/AcceptOffer on a listing that's already in locked state
    /// (post-buy, awaiting release). Seller already received payment;
    /// can't unwind.
    #[error("Listing is locked (already bought, awaiting release) — cannot modify")]
    ListingLocked {},

    /// Vesting duration exceeds the cap. Cap = ~1.9 years on Terra2 6s blocks.
    #[error("Vesting duration too long: max {cap} blocks (~1.9 years)")]
    TimeLockTooLong { cap: u64 },

    /// Whitelist size > MAX_WHITELIST_SLOTS.
    #[error("Whitelist too large: max {cap} entries")]
    WhitelistTooLarge { cap: u32 },

    /// Whitelist provided but with zero entries.
    #[error("Whitelist must have at least one entry — leave it None for an open listing")]
    WhitelistEmpty {},

    /// Whitelist entry is malformed: max_buys=0 or duplicate address.
    #[error("Invalid whitelist entry: max_buys must be ≥1 and addresses unique")]
    WhitelistInvalidEntry {},

    /// Caller tried to set BOTH whitelisted_buyer (V1.4 single-private) AND
    /// whitelist (V1.5 multi-address). Mutually exclusive — pick one.
    #[error("Set whitelisted_buyer OR whitelist — never both")]
    WhitelistAndPrivateConflict {},

    /// Buyer (or accepted offer's buyer) is not in the listing's whitelist.
    #[error("Your wallet is not on this listing's whitelist")]
    NotInWhitelist {},

    /// Buyer is on the whitelist but their slot is already exhausted.
    #[error("Your whitelist slot is exhausted")]
    WhitelistSlotExhausted {},
}
