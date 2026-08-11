# Atrium Marketplace — V1.6.1 Remediation Response

**Responds to:** SCV Security audit report v0.1, 2026-08-08
**Audited commit:** `a365b75` (v1.6.0-rc1)
**Remediation branch/version:** `audit-remediation` → v1.6.1-rc1
**Test suite:** 60 passing (was 57) — `cargo test`; builds clean for
`wasm32-unknown-unknown`.

Every code change is tagged in-source with an `AUDIT (finding N)` comment.

---

## 1. Discounted fee settlements spend funds held in escrow — SEVERE → **Fixed**

**Root cause** (`src/contract.rs`, `execute_sale`): treasury was computed as
`fee_amount * treasury_share_bps / effective_fee_bps`. Dividing by the
*discounted* effective fee made `treasury_share_bps / effective_fee_bps > 1` on
the Crystal/Cosmic tiers, so treasury was paid more than the whole fee and the
failed `fee - treasury` subtraction was masked with `unwrap_or(zero)`.

**Fix:**
- Treasury is now a fixed fraction of the collected fee:
  `treasury = fee_amount * treasury_share_bps / 10000`, `capa = fee − treasury`
  via `checked_sub` (no masking). Independent of the discount tier.
- `treasury_share_bps` is re-based to "fraction of fee out of 10000" and
  validated `≤ 10000` at instantiate / update_config / migrate through a new
  shared `validate_fee_config()`. This guarantees `treasury ≤ fee`, so the
  subtraction can never underflow.
- **Conservation invariant**: before emitting any transfer, `execute_sale`
  asserts `seller + treasury + capa + royalty == price` (new
  `ContractError::SettlementImbalance`) — a sale can never again pay out more
  than the buyer paid in.
- All four settlement paths (buy native / buy cw20 / accept_offer /
  accept_collection_offer) route through `execute_sale`, so the fix covers
  every path.

**Config migration:** deploy v1.6.1-rc1 and migrate with
`treasury_share_bps = 6660` — for the deployed non-holder fee of 500, that
reproduces the intended 66.6% treasury / 33.4% CAPA split exactly
(`333/500 == 6660/10000`), now applied correctly on every tier.

**Tests:** `audit_finding1_treasury_split_is_fraction_of_fee_no_escrow_drain`
(66.6% split, conservation, and an unrelated offer's escrow left untouched
by a sale) + `v16_settle_sale_uses_non_holder_rate_in_test_env`.

**Operational (not code):** the pre-existing on-chain shortfall (≈2.745 SOLID;
Offer ID 2's 30 SOLID refund under-collateralised) must be topped up to the
deployed contract before that refund is processed — this remediates the code,
not the already-drained balance.

---

## 2. Migration-provided fee values are ignored — LOW → **Fixed**

`migrate()` now loads Config and applies every `Some` field of `MigrateMsg`
(`fee_bps_non_holder` / `_crystal` / `_cosmic`, plus a new `treasury_share_bps`),
re-validates via `validate_fee_config()`, and saves. `None` leaves a field
unchanged — there is no silent default backfill, and the conflicting "backfills
with safe defaults" doc has been removed.

**Test:** `audit_finding2_migrate_applies_fee_config` (applies values; all-None
leaves them unchanged; out-of-range treasury share rejected at migrate-time).

---

## 3. Seller-only fee previews return the non-holder fee — LOW → **Fixed**

`get_effective_fee` + `effective_trade_tier` are unified into
`effective_fee_and_tier(buyer: Option<&Addr>, seller: Option<&Addr>)`, which
resolves the best-of-two tier from **whichever side(s) are supplied**. The
`FeeInfoForTrade` handler now calls it with both optionals, so a seller-only
preview applies the seller's tier instead of falling back to non_holder.
Settlement passes both sides and additionally now does a single crystal-tier
scan per side instead of two.

*(The Cosmic/Crystal branch can't be exercised in cw-multi-test because tier
resolution reads the hard-coded ALTAR/FUSION/MINT mainnet contracts; the
unified resolver is shared with settlement, which SCV validated on-chain.)*

---

## 4. Crystal tier resolution only checks the first 30 tokens — LOW → **Fixed**

`highest_crystal_tier` now paginates the owner's Crystals (`start_after` loop)
until a Cosmic is found (short-circuit) or the wallet is exhausted, bounded by
`MAX_TIER_PAGES = 20` (600 tokens) as a gas backstop — far above the largest
possible holding given the fixed Crystal supply, so every real holder is fully
scanned.

---

## 5. Two-step ownership transfer is not implemented — INFO → **Fixed**

`TransferOwnership` now only PROPOSES (writes `PENDING_OWNER`); the proposed
owner must call the new `AcceptOwnership {}` to take control. New errors
`NoPendingOwner` / `NotPendingOwner`. Re-proposing overwrites; proposing the
current owner cancels.

**Test:** `invariant_29_transfer_ownership_is_two_step`.

---

## Observation — Royalty terms can change after a listing is created → **Addressed (snapshot)**

Royalty is now **snapshotted into the `Listing` at creation** (new
`Listing.royalty: Option<RoyaltyInfo>`), and settlement prefers the snapshot
over the current collection royalty. A later admin `SetRoyalty` therefore
cannot change the proceeds a seller committed to. Pre-migration listings
(`None`) fall back to the current royalty for backwards compatibility. All
settlement paths run through the seller's listing, so offers and collection
offers inherit the snapshot too.

**Test:** `audit_royalty_snapshot_frozen_at_list_time`.

---

## Change summary

`src/contract.rs`, `src/state.rs`, `src/msg.rs`, `src/error.rs`,
`src/integration_tests.rs`. New: `validate_fee_config`, `effective_fee_and_tier`,
`execute_accept_ownership`, `PENDING_OWNER`, `SettlementImbalance` /
`No/NotPendingOwner`, `Listing.royalty`, `MAX_TREASURY_SHARE_BPS`,
`MAX_TIER_PAGES`. 60 tests passing.
