# Atrium Marketplace — Internal Audit (v1.2.0-rc1)

## V1.2.0 — Cosmic-only discount + 5% base fee (Daniel 2026-04-26)

Replaces V1.1.0's 5-rung Crystal tier ladder with a single sharp cliff:

| Buyer profile | fee_bps | Effective rate |
|---|---|---|
| Cosmic Crystal-holder | 0 | 0.00% (free) |
| Everyone else (incl. other Crystal tiers, no Crystal) | 500 | 5.00% (default) |

**Rationale:** Operator feedback (Scandalous-collection holder, 2026-04-26)
flagged that scaled discounts dilute the Cosmic premium and give discounts
nobody really values. Single sharp cliff makes Cosmic the apex perk and
keeps marketplace economics intact at modern marketplace fee-rates.

**Code change:**
```rust
fn get_effective_fee(deps: Deps, config: &Config, buyer: &Addr) -> StdResult<u16> {
    let highest = highest_crystal_tier(deps, buyer)?;
    if matches!(highest.as_deref(), Some("cosmic")) {
        return Ok(0);
    }
    Ok(config.fee_bps)  // 500 bps post update_config
}
```

`highest_crystal_tier` is unchanged — still surfaces highest tier for
FeeInfoResponse.crystal_tier so UI can show "you own a Charged Crystal"
badges (informational only — no fee discount).

**Migration phoenix-1:**
| | Värde |
|---|---|
| Wasm sha256 | `aa673620d8bbefcd3329ce619b8e3ec67eff0e43f83c012a74fda85131432bcc` |
| Wasm storlek | 374 KB |
| Optimizer | `cosmwasm/optimizer:0.16.0` |
| Store-tx | `1D59E97EFF0C13FD71CCE120591E84A14942DC9F29AA61D9A64063C41CC4D888` (h. 20724768) |
| Migrate-tx | `B0A7578A5FA88ED75A5758B180B8EC7D9BA1044C85DD81AFD2149C1F8224B34D` (h. 20724777) |
| update_config fee_bps | `E77F5533722EDA14CD9E84A8808D18458CF34D16471D906DE4DB72E8CC21A06E` (h. 20724785) |
| update_config treasury_share_bps | `FD6457E713249B33FA363F96D25F16C2AE4563460BBAD9B37620673370229452` (h. 20724792) |
| Old code_id | 3849 |
| New code_id | **3850** |
| Contract | `terra15du229lqcxkn939pmjgklqunftf604q4wz87kt5awj6reghec5jqs0w0kj` (oförändrad) |
| New fee_bps | **500 (5.00%)** |
| New treasury_share_bps | **333** (preserves ~2/3 split) |
| Migrate signer | `atrium-admin` (`terra1ef4g5x...`) — NOTE: peg-bot is NOT contract-admin |

**On-chain verification post-migrate (V1.2.0):**

| Wallet | Tier | fee_bps | discount_bps |
|---|---|---|---|
| `terra1cqc26l...` (val-key) | cosmic | 0 | 500 |
| `terra18hhej6...` (peg-bot) | radiant | 500 | 0 |
| anonymous (no Crystal) | None | 500 | 0 |

**In-flight listing-impact (acknowledged, accepted by operator):**
4 active listings at migrate time (#4 peg-bot 30 SOLID + #5/#6/#7 Scandalous
20 SOLID each). All listed under V1.1 fee_bps=150 expectations. After
fee_bps→500, sellers receive 95% net instead of 98.5%. Total max impact
across all 4 if sold immediately: ~3.15 SOLID (~$3 at $1/SOLID). Decision
documented in `audit F-06` (royalty-change-affects-in-flight) parent
finding — same admin-trust-bounded class.

**State preservation:** No Config-field schema changes. Same Listings,
Offers, Royalties, allowlist, caps. cw2 contract version updated.

---

## V1.1.0 — Crystal tier ladder ("Alt D") [DEPRECATED]

Replaces V1.0's binary 0% / 1.5% Crystal-holder check with a 5-rung ladder
based on the buyer's HIGHEST owned Crystal tier:

| Tier      | Fee bps | Effective rate |
|-----------|---------|----------------|
| Cosmic    | 0       | 0.00% (free)   |
| Prismatic | 25      | 0.25%          |
| Radiant   | 50      | 0.50%          |
| Charged   | 100     | 1.00%          |
| Raw       | fee_bps | 1.50% (default) |
| No Crystal | fee_bps | 1.50% (default) |

**New components:**
- `MigrateMsg {}` + `migrate(deps, env, _msg)` entry-point
- `highest_crystal_tier(deps, buyer)` — walks up to TIER_QUERY_LIMIT (30)
  Crystals owned by buyer, returns highest tier across ALTAR → FUSION → MINT
  resolution chain. Cosmic short-circuits.
- `resolve_tier(deps, altar, fusion, mint, token_id)` — per-token tier lookup,
  swallows individual contract errors and returns None.
- Hardcoded mainnet addresses for ALTAR/FUSION/MINT (zero migration surface,
  no admin-mis-set risk).

**Gas analysis:**
- Buyer with 0 Crystals: 1 query (cw721 Tokens) → ~25K extra gas
- Buyer with N≤30 non-Cosmic Crystals: 1 + 3N queries → up to ~2.3M extra gas
- Buyer with Cosmic at any position ≤30: short-circuits → ~50K-2.3M depending on position
- Whale (>30 Crystals) edge case: only first 30 by token_id_asc are checked.
  Crystals 1-50 ARE the original Cosmics → very low miss rate.

**FeeInfoResponse extended:** new `crystal_tier: Option<String>` field
populated with the highest tier (or `None`). `crystal_holder` kept for
backwards compat, derived from `crystal_tier.is_some()`.

**Tx events extended:** `buy_nft` action now emits `effective_fee_bps`
attribute alongside existing `fee` attribute, so off-chain indexers can
distinguish "1.5% paid because no discount applied" from "1.5% paid because
buyer is Charged-tier" etc.

**Migration tx:** `BBF4080049A80B2CB4DB05A11E884372C1BD56365776E99627E29D93845C66CF`
on phoenix-1 height 20720383. New code_id 3849 (was 3848). State preserved
(no Config field changes).

**Verified on-chain post-migrate:**
| Wallet | Tier | fee_bps | discount |
|---|---|---|---|
| terra10h28ny6... (Crystal #549) | charged | 100 | 50 |
| terra1cqc26l... (val-key) | cosmic | 0 | 150 |
| terra18hhej6... (peg-bot) | radiant | 50 | 100 |
| terra1vrjdx0... (no Crystal) | None | 150 | 0 |

---



**Auditor**: Claude (single-actor, six independent passes)
**Date**: 2026-04-25
**Scope**: `contracts/marketplace/src/{contract,state,msg,error,lib,integration_tests}.rs`
**Build**: `cargo test --lib` → **29/29 invariants PASS**
**Counterfactual**: this is a Claude-side audit. It does not replace a paid audit; it is the surgical first line of defense before progressive-cap mainnet launch.

---

## TL;DR — release recommendation

> **GO for testnet (pisco-1) → mainnet behind progressive caps.**
> No critical or high findings. Two medium findings are admin-trust-bounded.
> The contract's blast-radius is hard-capped by: (1) collection allowlist
> (admin-curated, V1 starts with Crystal only), (2) per-collection active-listing
> cap, (3) per-NFT active-offer cap, (4) admin pause switch, (5) max-fee 5%,
> max-royalty 15%, (6) instantiate-time invariants (`treasury_share ≤ fee`).

**Hormozi value-equation framing**: every finding below is scored on perceived
risk × likelihood × blast-radius. Two mediums, seven lows, five info — all
inside the acceptable risk envelope for a curated launch with progressive caps.

**Miller StoryBrand framing**: the user (the hero) is the seller listing or the
buyer purchasing. Every error message (in `error.rs`) tells them what went
wrong AND points at the recovery path — never a bare `Unauthorized`.

**Wallet isolation (operator directive 2026-04-25)**: Atrium uses three brand-new
keyring entries (`atrium-admin`, `atrium-treasury`, `atrium-capa-pool`) with
ZERO shared signing authority with `peg-bot`, `val-key`, `crystal-treasury` or
`validator-operator`. Same clinical-isolation pattern that crystal-treasury got
in April. Any redirection of `atrium-capa-pool` to Crystal-holder rewards is a
discretionary operator action — never automated, never contract-promised.

---

## Pass 1 — Stargaze diff & new attack surfaces

The marketplace is a CosmWasm `cw721+cw20` two-sided market in the spirit of
Stargaze v2. **What's new** (i.e. *Astral-only* attack surface):

| Δ | Description | Risk |
|---|---|---|
| Crystal-holder 0% fee | `is_crystal_holder()` queries `cw721::Tokens { owner, limit: 1 }` on every native buy. Trusts CAPA Crystal CW721 (locked, audited). | LOW |
| Curated allowlist | New admin path; `AddCollection` / `RemoveCollection`. Removed collections leave existing listings queryable+buyable (intentional — sellers aren't punished). | LOW |
| Per-collection / per-NFT caps | New counters `ACTIVE_LISTINGS_PER_COLLECTION`, `ACTIVE_OFFERS_PER_NFT`. Bumped on add, decremented on every removal path. | LOW |
| CW20 refund-on-mismatch | Atypical UX (Stargaze rejects with error); we refund and continue. Attached message; submessage atomicity preserves all-or-nothing. | LOW |
| Crystal-discount path | Adds external query in critical path. Malicious Crystal contract → buy reverts (acceptable; Crystal contract is admin-locked). | LOW |

---

## Pass 2 — Fresh-eyes re-read

Read the contract end-to-end as if for the first time. Findings:

- **F-03** [LOW] — `query_listings_by_collection` and `query_listings_by_seller`
  do filter-after-take, so `limit` doesn't translate to a stable page size.
  Bounded by 200/collection cap, so safe for V1. Document for V2 migration.
- **F-04** [INFO] — `query_offers_by_nft` accepts `start_after` parameter but
  ignores it (`_start_after`). Bounded by 20-offers/NFT cap → no DoS surface.
  Wire it for V2 if pagination is needed.
- **F-05** [LOW] — `execute_update_config` re-checks `treasury_share_bps ≤ fee_bps`
  invariant after partial update — verified at line 793-795. ✓ No drift.

---

## Pass 3 — Twenty attack scenarios

| # | Scenario | Result |
|---|---|---|
| A1 | Re-entrancy via cw20 receive → market.Receive → cw20.Send → … | **Safe** — CosmWasm submessages run after the parent returns; no synchronous re-entrancy. |
| A2 | Malicious cw721 minter mints duplicate token_id | **Safe** — cw721-base prevents duplicates. Mitigation: allowlist only audited cw721 contracts. |
| A3 | Pause race: admin pauses mid-tx | **Safe** — pause checked at every entrypoint. No exploitable ordering. |
| A4 | **F-06** [MEDIUM] Royalty changed mid-listing — buyer sees old, pays new | **Admin-trust-bounded** — admin is single trusted party. V2: lock royalty at listing creation. |
| A5 | Malicious cw721 returns wrong owner / panics on transfer | **Safe** — list_nft never queries owner; relies on cw721's atomic SendNft callback. Allowlist mitigation. |
| A6 | Malicious cw20 returns true on Transfer but transfers nothing | **Trust boundary** — V1 expects users to only accept native or CAPA. Listing-level seller chooses payment type. |
| A7 | Bank-send to a contract that rejects (treasury / capa_pool / royalty) | **Self-inflicted op-risk** — admin sets these addresses; if mis-configured, ALL buys revert. Not exploitable. |
| A8 | NFT transfer to a contract-buyer that rejects ReceiveNft | **Safe** — atomic; entire buy reverts including the buyer's funds. |
| A9 | Listing-cap bypass via remove-then-readd | **Safe** — counter tracks live listings, not allowlist state. |
| A10 | Allowlist bypass via direct cw721 SendNft | **Safe** — receive_nft check rejects → tx reverts → SendNft rolled back atomically. |
| A11 | Self-purchase via proxy contract | **Out of scope** — proxy is a distinct identity by design. |
| A12 | Offer + later list → accept_offer | **Works** — offer is on (collection, token_id); accept_offer finds the active listing. ✓ Tested. |
| A13 | **F-10** [MEDIUM] Stranded offers after sale | Three offers on NFT #1; sale via offer #2; offers #1 and #3 remain. Buyer #2 now owns the NFT. Offers #1 and #3 are **funds-locked but recoverable**: original buyers can `cancel_offer` (refund) or, after expiry, anyone can `withdraw_expired_offer`. **No fund loss.** UX cleanup: V2 could auto-cancel siblings on sale. |
| A14 | Withdraw_expired by stranger refunds to offerer | **Safe & tested** (invariant_22) — `build_refund_msg(&offer)` uses `offer.buyer`, never `info.sender`. |
| A15 | CW20 hook with wrong listing_id | **Safe** — buy_cw20 falls through to refund path. |
| A16 | Cancel listing for non-existent listing | **Safe** — `ListingNotFound`. |
| A17 | Tiny sale_price + integer division → fee = 0 | **Acceptable** — small sales pay no fee. No exploit; small-trade-griefing isn't economically rational at gas costs. |
| A18 | Royalty + fee = 100% sale_price | **Cap-protected** — max fee 5% + max royalty 15% = 20%; seller minimum 80%. The `FeeExceedsPrice` guard is defensive future-proofing. |
| A19 | Listing 0-price via Uint128 overflow | **Safe** — `is_zero()` check + Uint128 has no overflow. |
| A20 | Offer at higher price than listing → seller drains buyer | **By design** — offers are escrows; seller accepts at offer price; offer is a voluntary commitment by the buyer. Not exploitative. |

---

## Pass 4 — Invariant fuzz

Five state invariants verified by static analysis + tested:

- **I-1**: `ACTIVE_LISTINGS_PER_COLLECTION[c]` == count of listings in `LISTINGS` where `nft_contract == c`. **Holds**: bumped on `execute_receive_nft`; decremented on `execute_sale`, `execute_cancel_listing`, `execute_accept_offer` (via execute_sale).
- **I-2**: `ACTIVE_OFFERS_PER_NFT[(c, t)]` == count of offers in `OFFERS_BY_NFT[(c, t, *)]`. **Holds**: bumped on `make_offer_*`; decremented on `cancel_offer`, `accept_offer`, `withdraw_expired`.
- **I-3**: `seller_amount + treasury_amount + capa_amount + royalty_amount == sale_price` (exact integer split). **Holds** — verified by static math: fee_amount = sale × eff_fee/10000; treasury = fee × treasury_share/fee; capa = fee − treasury; seller = sale − fee − royalty. Edge cases tested (sale=1 with fee_bps=150 → fee=0, seller=1).
- **I-4**: After `cancel_listing`: NFT at seller, listing removed, ACTIVE_LISTING cleared. **Holds & tested** (invariant_17).
- **I-5**: After `cancel_offer`: buyer fully refunded, offer removed, counter decremented. **Holds & tested** (invariant_19).

No invariant violations found.

---

## Pass 5 — Gas / DoS analysis

Hot-path costs (CosmWasm gas units, rough order-of-magnitude):

| Path | Storage reads | Storage writes | External queries | Verdict |
|---|---|---|---|---|
| `list_nft` | 3 (config, caps, counter) | 3 (Listing, ACTIVE_LISTING, counter) | 0 | Cheap, O(1) |
| `buy_native` | 2 (config, Listing) | 2 removes + 1 dec | 1 (cw721 Tokens limit=1) | OK |
| `buy_cw20` | 2 (config, Listing) | 2 removes + 1 dec | 1 cw721 + 1 cw20 transfer | OK |
| `accept_offer` | 4 (offer, ACTIVE_LISTING, Listing, config) | 4 removes + 2 decs | 1 cw721 | OK |
| `query_all_listings` | up to 30 entries | 0 | 0 | OK (capped) |
| `query_listings_by_collection` | scans LISTINGS up to limit×n_filtered | 0 | 0 | **F-11** [LOW] linear scan (bounded by cap) |
| `query_offers_by_nft` | scans OFFERS_BY_NFT prefix | 0 | 0 | OK (offer cap = 20) |

**No DoS vector** — every loop is bounded by an admin-controlled cap or a paginated limit ≤ 30.

---

## Pass 6 — Public-bounty-style review

What would a public bounty hunter find?

- **B-01** [LOW] — `cancel_listing` by admin emits the same event as seller-cancel. Could distinguish. Cosmetic.
- **B-02** [INFO] — `expires_at` uses `block.height`, not Unix time. Predictable for sniping but standard pattern.
- **B-04** [LOW] — `from_json::<ListNftMsg>` is strict (`cw_serde` denies unknown fields by default). Malformed payloads → `InvalidCw721Msg`. ✓
- **B-05** [INFO] — `make_offer_cw20` accepts any cw20 contract; payment-type match enforced at `accept_offer`. Sellers must opt-in to a CW20 listing. ✓
- **B-06** [INFO] — Allowlist is fully admin-controlled; no on-chain verification of cw721 type. Mitigated by manual review pre-allowlisting.

---

## Findings summary

| ID | Severity | Title | Status |
|---|---|---|---|
| F-01 | LOW | Crystal-holder query gates every native buy | Accepted (Crystal contract locked) |
| F-02 | INFO | CW20 refund-on-mismatch novel pattern | Tested ✓ |
| F-03 | LOW | Filter-after-take in collection/seller queries | Bounded by cap; document for V2 |
| F-04 | INFO | `query_offers_by_nft` ignores start_after | Bounded by offer cap |
| F-05 | LOW | UpdateConfig re-checks invariant | Verified ✓ |
| **F-06** | **MEDIUM** | Royalty change affects in-flight listings | Admin-trust-bounded; V2 lock-at-creation |
| F-07 | LOW | No CW20 allowlist | Sellers opt-in per-listing |
| F-08 | LOW | Treasury contract that rejects bank breaks all sales | Self-inflicted op-risk |
| F-09 | n/a | (originally HIGH; cleared on re-analysis) | Atomic SendNft revert |
| **F-10** | **MEDIUM** | Stranded offers post-sale | No fund loss; cancel_offer or withdraw_expired works |
| F-11 | LOW | Linear scan in collection/seller queries | Bounded by cap |
| B-01 | LOW | Admin-cancel and seller-cancel emit same event | Cosmetic |
| B-02 | INFO | block.height not Unix time | Standard CosmWasm pattern |
| B-04 | LOW | Strict JSON deserialization | ✓ |
| B-05 | INFO | CW20 not allowlisted at offer time | Match enforced at accept |
| B-06 | INFO | Manual collection allowlist review | By design |

**Critical: 0** | **High: 0** | **Medium: 2** | **Low: 7** | **Info: 5**

---

## Recommended actions before mainnet

1. **Phase-1 mainnet caps (initial 30 days)**
   - `max_active_listings_per_collection: 50` (start strict, relax to 200 after 30d)
   - `max_active_offers_per_nft: 10` (start strict, relax to 20)
   - `initial_collections: [crystal]` (Crystal-only for first 30d)
   - `paused: false` but admin-ready to pause within seconds
2. **Pre-flight checks** (admin TODO before instantiate)
   - Treasury address is a wallet (not a contract) for V1 — eliminates F-08
   - CAPA reward address is the existing `crystal-treasury` LP-bound wallet
   - `crystal_nft_contract` matches the on-chain CAPA Crystal CW721
3. **Post-launch monitoring**
   - Sentinel rule: if `max_active_listings_per_collection - active_listings < 5` within 24h, alert (likely spam attempt)
   - Sentinel rule: alert on `paused: true` flips
   - Daily dashboard: total volume, fees collected, royalties paid, allowlist size
4. **V2 backlog** (post-30d, with paid audit)
   - Lock royalty at listing creation (closes F-06)
   - Auto-cancel sibling offers on sale (closes F-10 UX)
   - Indexed maps for collection/seller queries (closes F-03, F-11)
   - CW20 allowlist (closes F-07)
   - Auctions, collection bids
5. **Paid-audit funding**: when monthly volume exceeds $50K, operator may
   discretionarily allocate up to 10% of accumulated fee revenue (sitting in
   `atrium-treasury` and/or `atrium-capa-pool`) toward a paid CW2-style audit
   (Confio / OakSec). This is an operator decision — no contract clause and no
   automated allocation.

---

## Test coverage

29 invariant tests, **all passing** (`cargo test --lib`):

- Instantiation: 3 (success + fee cap + treasury_share invariant)
- Curation: 4 (allowed/disallowed listing, admin add, non-admin reject)
- Native buy: 5 (happy + self-purchase + inexact + multi-denom + expired)
- Crystal discount: 2 (zero-fee buy + fee-info query)
- CW20 buy: 2 (happy + self-purchase refund)
- Cancel listing: 2 (seller + stranger)
- Offers: 5 (make-cancel-refund + accept + payment-mismatch + expired-withdraw + never-expiring guard)
- Pause: 1 (paused blocks listing+offer)
- Caps: 1 (listing cap enforced + freed on cancel)
- Royalties: 1 (correct payout + 15% cap rejection)
- Misc: 3 (already-listed structurally enforced + zero-price reject + transfer ownership)

**Coverage gap (acceptable for V1)**: no Rust quickcheck/proptest fuzz; no formal verification.

---

## Sign-off

This contract is ready for **pisco-1 testnet deployment** and, conditional on
clean testnet smoke-testing (~7 days minimum), **mainnet behind the
Phase-1 caps above**.

— Claude, six-pass internal audit, 2026-04-25
