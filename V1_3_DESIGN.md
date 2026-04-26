# Atrium V1.3 — design doc (trait-aware offers + bulk + LST escrow)

Drafted 2026-04-26 in response to operator feedback (Scandalous-collection
holder). Three V1.3 features, ranked by complexity + risk.

> **Important:** V1.3 is NOT a code-now session. Each feature below
> needs ≥1 day of contract work + new integration tests + paid audit
> before mainnet because they introduce new escrow flows + new attack
> surface. This doc is the design + scope contract for that work.

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

## Feature C — LST on escrow (yield on parked funds)

### Problem

Per Scandalous-op:
> "Might be smart to enable a LST for the escrow account. Would be
> nice if your parking funds in escrow account it needs to be able to
> earn an APY while it's parked."

Both single-NFT offers and (especially) bulk collection offers can have
significant SOLID parked for days/weeks. Idle SOLID = lost yield.

### Design

When buyer makes an offer, instead of holding raw SOLID in marketplace,
swap it into ampSOLID (Eris-style auto-compounding LST) at offer-time.
At accept/cancel, swap back to SOLID and deliver.

**Atomicity problem:** Swap-back at accept-time can fail (slippage,
pool imbalance). If swap fails, accept fails — bad UX (seller wants
to accept their valid offer; can't because of pool conditions).

**Mitigation:**
1. Set a minimum slippage tolerance (default 1%, configurable per offer).
2. If swap fails, fallback to keep escrow in ampSOLID — seller receives
   ampSOLID instead of SOLID, can swap themselves.
3. UI shows "this offer parks in ampSOLID — accepting may take 2 txs
   if slippage spikes."

**Pricing risk for buyer:** ampSOLID/SOLID rate changes over time. If
buyer offers 100 SOLID and rate moves, they may pay slightly more or
less in actual SOLID terms. Disclosed at offer-time.

### Required infra

- ampSOLID contract address (or similar SOLID LST)
- Astroport SOLID/ampSOLID pool address
- Per-offer config: `enable_lst: bool` (opt-in default off in V1.3,
  flip to default on after burn-in)

### State changes

```rust
pub struct Offer {
    // ... existing fields ...
    pub escrow_denom: EscrowDenom,  // Native | Cw20 | LstAmpSolid
    pub original_amount: Uint128,   // for buyer's reference
}
```

### Gas + scope + RISK

- Code: +300 lines (swap msg construction, slippage handling, fallback)
- New attack surface: pool-manipulation, sandwich attacks at swap
- Tests: 10 new invariants (swap-fail handling, slippage edge cases,
  fallback path, oracle deviation)
- Estimated work: **8 hours impl + tests; 4 hours audit pass**
- **MUST have paid audit before mainnet** — first feature that
  introduces external pool dependencies.

---

## Cumulative scope

| Feature | Hours | Wasm size | Audit need |
|---|---|---|---|
| A — Trait-aware offers | 5 + 3 | +20 KB | Internal (Claude 6-pass) |
| B — Bulk SOLID escrow | 3 + 2 | +5 KB on A | Internal |
| C — LST escrow | 8 + 4 | +15 KB | **Paid audit required** |
| **Total** | **25 hours** | **+40 KB** | One paid pass |

---

## Recommended sequence

1. Merge Feature A first (next session, ~1 day) — unlocks both B and C
2. Merge Feature B next (~½ day) — instant value for collection projects
3. Hold Feature C until paid audit budget is allocated

V1.3.A could ship in 1 day; V1.3.B in 2 days. V1.3.C is a multi-week
project with paid audit.

---

## Status

- [x] Design doc drafted
- [ ] Daniel review + scope-confirm
- [ ] V1.3.A implementation
- [ ] V1.3.B implementation
- [ ] V1.3.C scoping + audit budget
