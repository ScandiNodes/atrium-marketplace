# Atrium · NFT marketplace contract

CosmWasm contract powering [`atrium.solidcapa.com`](https://atrium.solidcapa.com)
— a Terra2 NFT marketplace tuned to reward CAPA Crystal holders.

## Status

- **Live** on phoenix-1: `terra15du229lqcxkn939pmjgklqunftf604q4wz87kt5awj6reghec5jqs0w0kj`
- **Beta-mode**: Crystal-holder gated, single-collection (CAPA Crystals)
- **Pre-paid-audit** — internal review only; SCV / external audit pending
  before public multi-collection launch
- **Version:** [`1.6.0-rc1`](./AUDIT_V1.6.md) · code_id 3857
- **Build:** `wasm` artifact at `artifacts/atrium_marketplace.wasm` —
  sha256 `ebe461fddd15cda54f4781e1a730116bfc4b1df4e8b2150e546edc7eee6f922a`

## Fee model (V1.6.0)

A single fee per trade, debited from seller proceeds, where the **best
tier between buyer and seller** decides the rate:

| Configuration | Effective fee |
|---|---|
| Neither side holds a Crystal | 5.00% |
| At least one side holds any Crystal (non-Cosmic) | 1.50% |
| At least one side holds a Cosmic Crystal | 0.00% |

All three rates are admin-mutable via `UpdateConfig` without a
contract migration.

## Features

- Listings (native + cw20 payment)
- Direct offers + collection offers (V1.3)
- Trait-aware collection offers via merkle-proof trait registry (V1.3)
- Bulk floor-defense offers (V1.3)
- Private (whitelisted-buyer) listings (V1.4)
- Vesting / TLA-Lock listings (V1.5)
- Multi-address consumable-slot whitelists (V1.5)
- 3-tier best-of-buyer-seller fee schedule (V1.6)

See [`AUDIT_V1.6.md`](./AUDIT_V1.6.md) for the most-recent security
review and [`AUDIT.md`](./AUDIT.md) for the V1.5 baseline.

## Build

```bash
# Compile + run all 57 tests
cargo test --release

# Build optimized wasm artifact
docker run --rm \
  -v "$(pwd)":/code \
  -v atrium_v16_cache:/target \
  -v atrium_v16_registry:/usr/local/cargo/registry \
  cosmwasm/optimizer:0.16.0
```

## Repository

- `solid-online/atrium-marketplace` (this repo) — source of truth
- Live deployment metadata + audit history in [`AUDIT.md`](./AUDIT.md)
- V1.3 design doc: [`V1_3_DESIGN.md`](./V1_3_DESIGN.md)
- Keplr/DAODAO listing submissions: [`SUBMISSIONS_KEPLR_DAODAO.md`](./SUBMISSIONS_KEPLR_DAODAO.md)

## Authorship & disclosure

Built and operated by the Solid Protocol team. Co-authored across
multiple sessions with Claude (Anthropic) under operator supervision.
Commits aren't GPG-signed — operator can vouch for authorship for
audit teams that need a chain-of-custody.
