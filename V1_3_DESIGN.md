# Atrium V1.3 — design doc (trait-aware offers + bulk SOLID escrow)

Drafted 2026-04-26 in response to operator feedback (Scandalous-collection
holder). Two V1.3 features. **LST-on-escrow (originally Feature C) is
explicitly OUT of scope per Daniel 2026-04-26 — pool-dep risk + paid
audit budget not allocated.**

> Status: **shipping V1.3.0 in this session.** A + B implemented as one
> unified CollectionOffer struct (single-fill = max_trades=1, bulk =
> max_trades>1). Internal Claude audit only — no paid audit needed
> because no external pool deps, no swap-flow complexity.

---

## Feature A — Trait-aware collection offers

### Problem

Per Scandalous-op:
> "If you place a collection offer on aDAO and someone lists a broken NFT
> for cheap the collection offer could trigger and the buyer would be
> getting a NFT that's already been broken. Same with other traits — if
> you could place collection offer for say cosmic only way it would
> trigger was if a cosmic NFT triggered the sale."

Single-NFT offers are the only thing today. Need collection-level offers
that filter on traits at accept-time.

### Design

**New ExecuteMsg:**
```rust
ExecuteMsg::MakeCollectionOffer {
    nft_contract: String,
    /// Constraints any matching NFT must satisfy. Empty = any NFT.
    /// AND-semantics across constraints, OR-semantics within a single
    /// constraint's `accepted_values`.
    constraints: Vec<TraitConstraint>,
    /// max-trades = how many NFTs this offer is willing to buy before
    /// auto-closing. 1 for single-fill, N for bulk (Feature B below).
    max_trades: u32,
    expires_in_blocks: u64,
}

#[cw_serde]
pub struct TraitConstraint {
    /// Trait name as it appears in the cw721 metadata.attributes.
    /// Examples: "Status", "Tier", "Background"
    pub trait_type: String,
    /// Accepted values (OR-semantics within a constraint).
    /// Examples: ["Unbroken"], ["Cosmic", "Prismatic"]
    pub accepted_values: Vec<String>,
}
```

**New AcceptCollectionOffer flow:**
```rust
ExecuteMsg::AcceptCollectionOffer {
    offer_id: u64,
    /// Which token the seller is offering up to fulfil this collection
    /// offer. Contract verifies token's traits match constraints.
    token_id: String,
}
```

**Trait verification (the hard part):**
Two implementation paths:

#### Path 1 — query-time on-chain (simpler, gas-heavy)

At accept-time, query the cw721's `nft_info` query to get
`metadata.token_uri` → fetch off-chain JSON to get traits.

**Problem:** can't fetch HTTP from CosmWasm. Off-chain trait data is
not available to the contract.

**Resolution:** require collections to have on-chain trait data via a
sibling registry contract. Atrium maintains its own `TraitRegistry`
that stores per-token trait maps for each allowlisted collection.
Updated by the collection's admin.

#### Path 2 — off-chain trait snapshot + signed merkle proof

At allowlist time, the collection admin uploads a merkle root of
`{token_id, traits}` pairs. At accept-time, seller submits the merkle
proof for their token. Contract verifies the proof + checks
constraints.

**Pros:** O(log n) gas, trait data lives off-chain (fits CosmWasm
constraints), supports very large collections.
**Cons:** trait updates require new merkle root + new on-chain anchor.

**Recommended:** Path 2. Merkle proofs are well-known in CosmWasm
land (used in airdrops). Adds ~3KB wasm and ~50K gas per accept.

### State changes

```rust
// New storage
pub const COLLECTION_OFFERS: Map<u64, CollectionOffer> = Map::new("collection_offers");
pub const COLLECTION_OFFER_COUNT: Item<u64> = Item::new("collection_offer_count");

// Trait registry per collection
#[cw_serde]
pub struct TraitRegistry {
    pub merkle_root: [u8; 32],
    pub updated_at: u64,
    pub updated_by: Addr,
}
pub const TRAIT_REGISTRY: Map<&str, TraitRegistry> = Map::new("trait_registry");
```

### Gas + scope

- Code: +400 lines (state + msg + accept_collection_offer + trait verification + tests)
- Wasm size: +20 KB
- Gas-per-accept: +50K (merkle proof verification)
- Tests needed: 8 new invariants
- Estimated work: **5 hours design + impl + tests; 3 hours audit pass**

---

## Feature B — Bulk SOLID collection offer (escrow-based)

### Problem

Per Scandalous-op:
> "Load up our aDAO escrow account with 2,500 Solid and place a bulk
> collection offer for any unbroken aDAO NFT that gets listed for say
> 2x backing so 20 solid. We could basically set a floor price for our
> collection with bulk buy back offer that would keep buying until the
> escrow account hits zero."

Functionality: a project DAO loads escrow with X SOLID and offers to
buy N copies of any NFT in their collection that meets a price ceiling
+ trait constraints. As listings hit the price, the escrow drains.

This is **a built-in floor-price-defense mechanism**. Could become
Atrium's signature feature (no other Cosmos NFT marketplace has it).

### Design (assumes Feature A is in place)

Reuses `MakeCollectionOffer` with:
- `max_trades > 1` → multi-fill offer (bulk)
- An additional `price_per_nft: Uint128` field
- Total escrow at create-time = `max_trades * price_per_nft`
- Each `AcceptCollectionOffer` debits one slot from `max_trades` and
  releases one `price_per_nft` from escrow

**Auto-trigger via keeper bot (off-chain):**
The contract itself doesn't auto-fill on listings (gas + atomic-listing
problem). Off-chain keeper bot watches `list_nft` events → if listing
matches an open collection-offer's constraints + price, keeper calls
`AcceptCollectionOffer` on behalf of the seller (with seller's prior
approval to delegate).

**Alternative — seller-initiated only:** sellers see "match offer
detected" badge in UI when listing, click to accept. Simpler, no bot
required, but slower.

**Recommended:** Start with seller-initiated. Add keeper bot in V1.4
once volume justifies it.

### State changes (incremental on top of Feature A)

```rust
pub struct CollectionOffer {
    // ... existing fields ...
    pub price_per_nft: Uint128,
    pub max_trades: u32,
    pub trades_filled: u32,
    pub escrow_balance: Uint128,
    pub payment: PaymentType,
}
```

### Gas + scope

- Code: +200 lines on top of Feature A
- Tests: 6 new invariants (multi-fill, escrow drain, partial-fill cancel)
- Estimated work: **3 hours impl + tests (assumes Feature A done)**

---

## Feature C (REMOVED) — LST on escrow

**Decision 2026-04-26 (Daniel): out of scope.** Pool-dep + sandwich
attack surface too large for unaudited release; paid audit budget not
allocated. May revisit when SOLID LP-yield matures + budget exists.

---

## Cumulative scope (A + B only)

| Feature | Hours | Wasm size | Audit need |
|---|---|---|---|
| A — Trait-aware offers | 5 + 3 | +20 KB | Internal (Claude 6-pass) |
| B — Bulk SOLID escrow | 3 + 2 | +5 KB on A | Internal |
| **Total** | **13 hours** | **+25 KB** | Internal only |

Implementation note: A and B share state struct. `CollectionOffer.max_trades=1`
is single-fill (Feature A); `max_trades>1` is bulk (Feature B). One execute
flow, one cancel flow, one query path.

---

## Status

- [x] Design doc drafted
- [x] Daniel scope-confirm — A + B yes, C dropped
- [ ] V1.3.0 implementation in progress (this session)
