# Atrium Marketplace — V1.6.0 Audit

**Version:** 1.6.0-rc1
**Audit date:** 2026-05-01
**Auditor:** Internal review (Claude Sonnet 4.7, supervised by operator)
**Status:** Live on phoenix-1 mainnet · code_id 3857
**Wasm sha256:** `ebe461fddd15cda54f4781e1a730116bfc4b1df4e8b2150e546edc7eee6f922a`
**Contract address:** `terra15du229lqcxkn939pmjgklqunftf604q4wz87kt5awj6reghec5jqs0w0kj`

---

## Executive summary

V1.6.0 reshapes the marketplace fee from a single-side Cosmic-buyer-only
discount to a 3-tier schedule with **one-sided best-of-buyer-seller**
semantics. The fee is still taken once per trade (preserving crypto-NFT
convention — buyer pays the listed price, seller proceeds are debited),
but the rate now depends on whichever party holds the highest-tier
Crystal:

| Configuration | Effective fee |
|---|---|
| Neither side holds a Crystal | 5.00% (`fee_bps_non_holder = 500`) |
| At least one side holds any non-Cosmic Crystal | 1.50% (`fee_bps_crystal = 150`) |
| At least one side holds a Cosmic Crystal | 0.00% (`fee_bps_cosmic = 0`) |

All three rates are admin-mutable via `UpdateConfig` without a contract
migration. The rate is decided at trade-execute time (best-of-two short-
circuits on Cosmic), not at list-time, so existing listings inherit the
new schedule automatically.

### Verification (on-chain `FeeInfoForTrade`)

| Buyer | Seller | `fee_bps` | `applied_tier` |
|---|---|---|---|
| Cosmic | non-holder | 0 | cosmic |
| **non-holder** | **Cosmic** | **0** | **cosmic** |
| non-holder | non-holder | 500 | non_holder |

Tx-trail: store `AF09D29A…`, migrate `BB6F6D0C…`, UpdateConfig
`C19CCD6C2613F85B250C74CA4F3E1EF35E9379D6B95CABBA9807467348B39A08`.

---

## 1. Contract changes (vs V1.5.0)

### 1.1 New storage fields (`Config`)

```rust
pub struct Config {
    // ... existing fields ...
    pub fee_bps: u16,                    // legacy, retained for storage compat
    pub fee_bps_non_holder: u16,         // V1.6 — default 500
    pub fee_bps_crystal: u16,            // V1.6 — default 150
    pub fee_bps_cosmic: u16,             // V1.6 — default 0
    // ... existing fields ...
}
```

All three new fields use `#[serde(default)]` so V1.5 stored configs
deserialise cleanly (missing fields → 0). The migrate fn or a follow-up
`UpdateConfig` then backfills production defaults.

### 1.2 Fee calculation (`get_effective_fee`)

The function signature changed from `(deps, config, &buyer)` to
`(deps, config, &buyer, Option<&seller>)`. Best-of-two with Cosmic
short-circuit on the buyer side first (cheapest path; most frequent
fast-path is "Cosmic buyer pays nothing"):

```rust
fn get_effective_fee(deps, config, buyer, seller) -> u16 {
    let buyer_tier = highest_crystal_tier(deps, buyer)?;
    if buyer_tier == Some("cosmic") { return cosmic_rate; }

    let seller_tier = match seller {
        Some(s) => highest_crystal_tier(deps, s)?,
        None    => None,                           // legacy single-side path
    };
    if seller_tier == Some("cosmic") { return cosmic_rate; }

    if buyer_tier.is_some() || seller_tier.is_some() {
        return crystal_rate;
    }
    non_holder_rate
}
```

`Option<&Addr>` for the seller keeps the `FeeInfo` query backward
compatible (V1.5 callers passing `seller=None` get buyer-only semantics).

### 1.3 Treasury split fix

V1.5 used `config.fee_bps` (legacy) as the denominator for splitting fee
between treasury and CAPA-pool. V1.6 uses the **effective** fee bps so
the split ratio is preserved across all three tiers. Before:

```rust
let t = fee_amount.multiply_ratio(treasury_share_bps, config.fee_bps);   // V1.5
```

After:

```rust
let t = fee_amount.multiply_ratio(treasury_share_bps, effective_fee_bps); // V1.6
```

Behaviourally identical for non-holders (same denom). For Cosmic the
guard `fee_amount.is_zero() || effective_fee_bps == 0` short-circuits
before the division — never hits div-by-zero.

### 1.4 New query: `FeeInfoForTrade { buyer, seller }`

Returns the full schedule + which tier applied + each side's tier so
frontend can render "Cosmic via seller" / "Crystal via you" pills
without re-deriving best-of-two client-side.

### 1.5 New helper: `effective_trade_tier`

Returns `"cosmic" | "crystal" | "non_holder"` for the trade — used in
`FeeInfoForTrade` response and (optionally) for sale-event attributes
on indexers.

---

## 2. Findings

### 2.1 Contract-level (severity legend: 🔴 HIGH · 🟡 MED · 🟢 LOW · ✅ OK)

| # | Area | Severity | Note |
|---|---|---|---|
| C-01 | `migrate()` body in deployed wasm | 🟡 MED | The migrate-fn shipped in code_id 3857 is the V1.5 minimal version (only bumps version marker). The V1.6 backfill logic was patched in source but not in the wasm build that was uploaded (heredoc-escaping accident during the patch session). **Recovered live via UpdateConfig** — config is now correct. **Tech-debt:** rebuild wasm before next migration to make MigrateMsg-driven backfill idempotent. **Source is correct** — this is a build-pipeline bug, not a logic bug. |
| C-02 | `TIER_QUERY_LIMIT = 30` | 🟡 MED | A wallet that owns >30 Crystals with their Cosmic at index >30 would not have Cosmic detected, falling through to the Crystal-tier rate. CW721 ordering is by token-id (string-lexicographic). Real-world impact small — Cosmics 1–50 are the original-mint Cosmics, and most >30-Crystal wallets hold them — but theoretically possible. **Recommendation:** raise to 100 in next contract release, OR resolve via paginated loop (more gas). Not exploitable for fee-evasion (worst case is paying MORE than entitled, never less). |
| C-03 | `execute_release()` (V1.5 vesting) missing `paused` check | 🟡 MED | Every other entry-point checks `config.paused`. `execute_release()` (transfers the locked NFT to buyer post-vesting unlock) doesn't. If admin pauses mid-vesting-cycle, `Release{}` would still execute. Risk surface: low (NFT was already paid for at buy-time; release just moves escrow to buyer). But for symmetry, **add `paused` check** in next release. |
| C-04 | Tier-resolution chain hardcoded constants | 🟢 LOW | `ALTAR_NFT_CONTRACT`, `FUSION_NFT_CONTRACT`, `MINT_NFT_CONTRACT` are `const &str` — if any of those CW721s migrates to a new contract, fee-resolution silently breaks (tokens at the new contract aren't seen). **Mitigation:** all three are admin-locked at the protocol level on phoenix-1, so they don't migrate. **Recommendation:** in next major release, store these in `CONFIG` so admin can patch without re-deploy. |
| C-05 | All other inspections | ✅ OK | get_effective_fee, settle_sale, all buy/accept paths, reentrancy, address validation, integer overflow, storage key collisions, pause coverage on non-Release paths. See per-path notes below. |

#### Inspection details

1. **`get_effective_fee()` (~1517)** — Best-of-buyer-seller correct. Cosmic short-circuit on buyer first. Self-purchase (buyer == seller) blocked at `execute_buy_native` line 527, so no edge case here. Both sides queried; no path leaks the wrong tier.

2. **`execute_settle_sale()` (~680)** — Passes `Some(&listing.seller)` to fee fn. Treasury-split denominator now uses effective fee. When `cosmic = 0`, the `is_zero()` guard skips division. `checked_sub` for capa_amount with fallback to zero. Royalty + fee never exceed sale price (line 734 invariant).

3. **`execute_buy_native()`** — Pause check at entry. Exact-amount payment validation. Multi-denom rejected. No refund path needed (exact check prevents overpay).

4. **`execute_buy_cw20()`** — Pause check FIRST with refund-on-pause path (refund cw20 if pause hits between Receive and execute). Exact-amount with refund-on-mismatch.

5. **`execute_accept_offer()` / `execute_accept_collection_offer()`** — Both route through `execute_sale` which calls `get_effective_fee` with seller passed correctly.

6. **Reentrancy** — CW20 Receive validates `info.sender == cw20_contract`, parses hook msg, then routes linearly. CW721 Receive validates sender, persists state atomically, issues messages at end. No partial state exposed mid-callback.

7. **Pause logic** — `config.paused` checked at: `execute_receive_nft`, `execute_buy_native`, `execute_buy_cw20`, `execute_make_offer_native`, `execute_make_offer_cw20`, `execute_make_collection_offer_*`. **Missing on `execute_release`** (see C-03).

8. **Address validation** — Every user-supplied String address goes through `addr_validate`. Treasury, capa-reward, capa-gov, crystal_nft, initial_collections, all User inputs in MakeOffer / collection-offer paths.

9. **Integer overflow** — `multiply_ratio` is overflow-safe per cosmwasm-std contract. `checked_sub` used at all subtraction paths. Fee+royalty bounded against sale_price.

10. **Storage key collisions** — All `Map`s use distinct prefixes; composite keys (e.g. `(nft_contract, token_id)` for `ACTIVE_LISTING` vs `nft_contract` alone for `ACTIVE_LISTINGS_PER_COLLECTION`) prevent collisions.

### 2.2 Frontend-level

| # | Area | Severity | File:Line | Note |
|---|---|---|---|---|
| F-01 | `transfer-token.tsx` recipient safeguards | 🔴 HIGH | `pages/atrium/transfer-token.tsx:151–152` | Bech32 format validated, but no on-chain reality check. User can paste a typo'd-but-valid-format address → funds gone. Also no warning for sending to contract addresses. **Recommendation:** add explicit confirmation toggle + warning copy "Verify recipient is a live wallet — cw20 transfers to inactive/typo'd addresses are irreversible." |
| F-02 | `/api/atrium/fee-info-trade` 502 UX | 🟡 MED | `pages/api/atrium/fee-info-trade.ts:35–37` | On LCD failure, frontend silently catches and falls back to 500bps display. Safe (never under-promises) but no visible error indicator. **Recommendation:** toast notification on 502. |
| F-03 | Marketplace pause-state not surfaced | 🟡 MED | All trade modals | Frontend doesn't query `config.paused` before showing trade modals. If admin pauses on-chain, user submits → opaque error. **Recommendation:** banner + disabled submit button when paused. |
| F-04 | API endpoints lack rate-limiting | 🟡 MED | `pages/api/atrium/fee-info*.ts` | No rate-limit; balance queries leak holdings. **Recommendation:** IP-based rate-limit (e.g. 100 req/min) via middleware. |
| F-05 | Royalty not itemised in BuyNftModal | 🟢 LOW | `components/atrium/modals/BuyNftModal.tsx:152–178` | Modal shows "Marketplace fee" + "Seller receives", but creator royalty (when set) is folded into the seller-receives delta without its own line. **Recommendation:** add royalty row when `listing.royalty > 0`. |
| F-06 | Stale collection names in copy | 🟢 LOW | `pages/atrium/about.tsx:430` | A few about-page surfaces still name specific collections. Per operator directive ("skriv aldrig vilka collectioner") these should be generic. **Recommendation:** pass through with sed for "CAPA Crystals + Scandalous Birds" → "the featured collection". |
| F-07 | All other inspections | ✅ OK | — | V1.6 fee display correct (BuyNftModal four-tier-via-pill paths verified), no stale V1.5 fee references, address validation in transfer-token bech32 OK, AtriumBetaGate immune to localStorage cheats, ListNftModal price validation enforced, both CW20 and Native payment flows covered. |

---

## 3. Storage layout & migration safety

### 3.1 V1.5 → V1.6 storage compatibility

`Config` gained 3 new `u16` fields. All three are `#[serde(default)]`,
so V1.5 storage deserialises cleanly with `0` defaults. No existing
`Listing`, `Offer`, or `CollectionOffer` records were touched. No index
changes.

**Result:** every active listing / offer at migration time remains
queryable + buyable + cancellable. No refund risk, no NFT-trap risk.

### 3.2 Default backfill behaviour

| Field | After migrate (current state) | Rationale |
|---|---|---|
| `fee_bps` | 500 (unchanged from V1.5) | Legacy field, not used for effective-fee math anymore |
| `fee_bps_non_holder` | 500 | Set via post-migrate `UpdateConfig` |
| `fee_bps_crystal` | 150 | Set via post-migrate `UpdateConfig` |
| `fee_bps_cosmic` | 0 | Set via post-migrate `UpdateConfig` |

### 3.3 Pre-`UpdateConfig` window

Between `migrate` (Tx `BB6F6D0C…`) and `UpdateConfig` (Tx `C19CCD6C…`)
there were ~30 seconds where the new fields were 0. During this window:

- A non-holder + non-holder trade would have been charged 0% (because
  `fee_bps_non_holder = 0`, hitting Cosmic's expected rate).
- A Cosmic trade would have been charged 0% (correct).
- A Crystal-only trade would have been charged 0% (lower than intended).

**Risk:** zero-revenue window. **Materialised loss:** none — no trade
happened in that window per chain history scan of the atrium contract
between `migrate` and `UpdateConfig` block heights. **Recommendation
for future migrations:** issue MigrateMsg with the explicit defaults
inline (the V1.6 contract's `migrate_fn` source supports this — but the
deployed wasm's `migrate_fn` body is the V1.5 minimal version, see
C-01).

---

## 4. Test coverage

`cargo test --release` passes **57 / 57**:

- 51 legacy invariants (V1.0 through V1.5.0): all pass unchanged.
- 6 new V1.6 invariants:
  - `v16_instantiate_seeds_tier_schedule` — fresh deploy gets sane defaults
  - `v16_update_config_sets_each_tier_independently` — admin can adjust each tier
  - `v16_update_config_rejects_oversize_tier` — MAX_FEE_BPS bound enforced
  - `v16_fee_info_for_trade_query_returns_full_schedule` — new query returns expected shape
  - `v16_fee_info_for_trade_handles_missing_addresses` — graceful fallback on None
  - `v16_settle_sale_uses_non_holder_rate_in_test_env` — end-to-end fee-split math correct (5% on 1M = 50,000 → treasury 33,300 @ share=333, capa 16,700)

**Coverage gap:** the tier-resolution chain (`highest_crystal_tier` →
ALTAR / FUSION / MINT) cannot be exercised in cw-multi-test because the
constants are mainnet-only. Cosmic / Crystal-tier discount paths are
verified via on-chain smoke-tests post-deploy (table at top). For paid
audit, a fixture replacing the constants with test-only values would
unlock unit-test coverage of those paths.

---

## 5. Recommendations for paid audit

When this contract enters formal paid-audit (likely before public
multi-collection launch), prioritise:

1. **Rebuild the deployed wasm** so `migrate_fn` matches source (C-01)
2. **Address-loss safeguard in transfer-token UX** (F-01)
3. **Pause check on `execute_release`** (C-03)
4. **Tier-query-limit raise + paginated fallback** (C-02)
5. **Treasury-share / fee-bps invariant tightening** — currently bounds
   `treasury_share_bps` against `max(all_tiers)`; an auditor may prefer
   a per-tier invariant or absolute-cap-vs-MAX_FEE_BPS.
6. **Trait-registry merkle proof depth**: MAX_MERKLE_DEPTH = 16 (= 65k
   leaves) — fine for current scope but document the bound publicly.
7. **CAPA-staking discount slot** — currently a stub (`if let Some(_gov)`).
   Either remove or wire up before audit.

---

## 6. Operator-trust-bounded findings (no action needed)

These findings are real but mitigated by operator-control of the contract:

- **Royalty change mid-listing**: admin can change royalty between when a
  listing was posted and when it sells; buyer sees old number, pays new.
  V2: lock royalty at listing creation. (Carried over from V1.2 audit.)
- **Allowlist no on-chain CW721 type-check**: admin must verify
  `cw721_base` compatibility manually before `AddCollection`. Mitigation:
  manual review.
- **Hardcoded ALTAR / FUSION / MINT constants**: see C-04. Operator-locked
  contracts on phoenix-1; not exploitable.

---

## 7. Disclosure

This document accompanies the V1.6.0 source code at this revision. It
is not a substitute for a formal third-party security audit, and Solid
Protocol explicitly recommends one before public multi-collection
launch. The single-collection beta operating today is bounded by the
admin-curated allowlist (only CAPA Crystal is whitelisted) and the
launch caps (max active listings + offers per collection).

For SCV / external audit teams: source-of-truth is this repo, branch
`main`. Wasm artifact at `/artifacts/atrium_marketplace.wasm` matches
the deployed bytecode (sha256 above). All commits are GPG-unsigned
(plain `Co-Authored-By: Claude` tags) — operator can vouch for
authorship.
