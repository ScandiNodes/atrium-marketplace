# Atrium — Keplr Apps + DAODAO submission package

Both submissions need a human (Daniel) to fill the form/PR — but every
field-value below is pre-baked. Copy-paste only.

---

## 1. Keplr Apps registry submission

**What it is:** the in-Keplr "Apps" browser tab. Adding Solid here puts
the protocol next to Stargaze / Liker.Land in the curated app drawer.
That bypasses Google search (clones), giving users a verified-URL
trust signal.

**Method:** PR to `chainapsis/keplr-extension` repo.
File: `packages/extension/src/pages/registry/registry.json`
(Or whatever the current path is — submission template below.)

### Pre-filled JSON entry

```json
{
  "name": "Solid Protocol",
  "description": "Lending, borrowing and NFT marketplace on Terra2. Mint your CAPA Crystal, deposit collateral, take SOLID-stablecoin loans, and trade NFTs on Atrium.",
  "url": "https://app.solidcapa.com",
  "logo": "https://app.solidcapa.com/icon-512.png",
  "twitter": "https://twitter.com/SolidCapaPult",
  "tags": ["DeFi", "Lending", "NFT", "Marketplace", "Terra2"],
  "chains": ["phoenix-1"]
}
```

### PR text (copy-paste body)

```markdown
## Add Solid Protocol to the Keplr Apps registry

Solid Protocol is a CDP-style lending protocol on Terra2 (phoenix-1)
with a SOLID stablecoin, CAPA governance token, the CAPA Crystals NFT
collection, and the Atrium NFT marketplace. Live since June 2025;
≥6 months of operation; non-custodial.

- **App URL:** https://app.solidcapa.com
- **Twitter:** https://twitter.com/SolidCapaPult
- **Discord:** https://discord.gg/EXxBfhEz28
- **Open-source contracts:** https://github.com/solid-online/atrium-marketplace
                              https://github.com/solid-online/capa-money-market
- **TVL:** ~$X (DefiLlama listing pending — PR #18830)

The Atrium marketplace just shipped V1.2 (5% fee, 0% for Cosmic Crystal
holders). Adding to Keplr Apps so users can reach the verified URL
without risk of phishing clones.

Happy to provide additional materials.
```

### Checklist before submit

- [ ] Verify icon-512.png exists at `app.solidcapa.com/icon-512.png` — if
      missing, generate from existing logo and add to `private-webapp/public/`
- [ ] Confirm Twitter handle is current (used `SolidCapaPult` — verify)
- [ ] Open PR against `chainapsis/keplr-extension` (find the registry file)
- [ ] Link the PR back to this file when merged

---

## 2. DAODAO submission

**What it is:** DAODAO is the cross-Cosmos DAO directory. Listing Solid
there gives discoverability + brings DAO-curious users into the protocol.

**Method:** Submit form at `daodao.zone/dao/[chain]/add` — needs DAO
chain-id + treasury address + branding.

### Pre-filled fields

| Field | Value |
|---|---|
| **Name** | Solid Protocol |
| **Description** | Lending, borrowing, and NFT marketplace on Terra2. Mint a CAPA Crystal, deposit collateral, borrow SOLID stablecoin, and trade NFTs on Atrium. |
| **Chain** | phoenix-1 (Terra2) |
| **DAO contract** | `terra1...` (TBD — need the gov contract address) |
| **Treasury** | `terra1...` (use protocol-treasury, NOT operator wallets) |
| **Logo** | https://app.solidcapa.com/icon-512.png |
| **Banner** | https://app.solidcapa.com/og-default.png |
| **Website** | https://app.solidcapa.com |
| **Twitter** | https://twitter.com/SolidCapaPult |
| **Discord** | https://discord.gg/EXxBfhEz28 |
| **Tags** | DeFi, Lending, NFT, Marketplace, Stablecoin |

### Checklist before submit

- [ ] Confirm DAO contract address — DAODAO usually expects an actual
      cw-dao-core or similar; if Solid's governance is just CAPA-stake
      voting via the CAPA token, may not qualify for the standard form.
      In that case, submit as a "Project" not a DAO if DAODAO supports it.
- [ ] Pull latest banner from public-landing-page repo
- [ ] Confirm the form at https://daodao.zone (UI may have moved)

### Risk note

DAODAO's form may reject non-DAO-core submissions. If so, the fallback
is to pitch as a featured-project listing via their Discord moderators.

---

## Status

Templates ready as of 2026-04-26. Both submissions require a human to
hit "submit" — Claude cannot complete browser forms or open PRs from
this session. Stamp the [done] box per submission when shipped.

- [ ] Keplr Apps PR opened
- [ ] DAODAO submission filed
