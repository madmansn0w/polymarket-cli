# Postmortem: Proxy Wallet Address Mismatch

**Date:** 2026-03-18
**Severity:** Critical (would have sent funds to wrong address)
**Status:** Resolved

---

## Background: How Polymarket Wallets Work

Polymarket does not let users trade directly from their Externally Owned Account (EOA — the address you see in MetaMask). Instead, it creates an intermediary smart contract wallet on Polygon for each user. This intermediary wallet is what actually holds your USDC, owns your positions, and signs orders on the CLOB.

There are two reasons for this architecture:

1. **Gas abstraction.** The intermediary wallet can batch multiple operations into a single transaction, and Polymarket can sponsor gas fees so users never need MATIC.
2. **Security isolation.** If Polymarket's web frontend is compromised, the attacker can only interact with the intermediary wallet's limited interface, not drain the user's entire EOA.

### Two Types of Intermediary Wallets

Polymarket has used two different smart contract systems for these intermediary wallets over time:

#### Proxy Wallets (Legacy)

The original system. Polymarket deployed a lightweight proxy contract per user using CREATE2 (a deterministic deployment method where the contract address is computed from the deployer address, a salt derived from the user's EOA, and the proxy bytecode). The proxy delegates all calls to a shared implementation contract.

- **Derivation function:** `derive_proxy_wallet(eoa, chain_id)`
- **Used by:** Early Polymarket accounts, the `polymarket-cli`'s default configuration
- **Signature type flag:** `--signature-type proxy`

#### Gnosis Safe Wallets (Current)

The current system for accounts created through the Polymarket website. Polymarket deploys a 1-of-1 Gnosis Safe multisig per user, also via CREATE2. The Safe is a battle-tested smart contract wallet used across the Ethereum ecosystem, with more features (batching, modules, guards) than the legacy proxy.

- **Derivation function:** `derive_safe_wallet(eoa, chain_id)`
- **Used by:** All website-created accounts (the vast majority of Polymarket users)
- **Signature type flag:** `--signature-type gnosis-safe`

### Why the Derivation Matters

Because both wallet types use CREATE2, the address is **deterministic** — it can be computed offline from the user's EOA without querying any API. The SDK provides both derivation functions. The CLOB authentication flow uses the derived address as the "funder" — the identity that owns positions and balances on the exchange.

If you derive the wrong type, you get a completely different address, and the CLOB authenticates you as a nonexistent account.

### How CLOB Authentication Works

The Polymarket CLOB (Central Limit Order Book) uses a two-level authentication system:

1. **L1 Auth (EIP-712 signature):** Your EOA signs a typed data message. This proves you own the private key. Used once to create/derive API credentials.
2. **L2 Auth (HMAC-SHA256):** All subsequent API calls use HMAC signing with the API key/secret/passphrase obtained in L1. This is fast and doesn't require the private key for every request.

During L2 authentication, the SDK must tell the CLOB which "funder" address to associate with the session. This is where the derivation happens:

```
EOA private key
    → derive funder address (proxy OR safe, based on signature_type)
    → authenticate with CLOB using funder as identity
    → all balance/order/position queries scope to that funder
```

---

## The Problem

We imported a private key into the CLI with the default `--signature-type proxy`. The Polymarket website had created a Gnosis Safe wallet for this EOA. The two derivations produce completely different addresses:

| Derivation | Address | Matches Polymarket? |
|-----------|---------|:---:|
| `derive_proxy_wallet(EOA)` | `0x8d5b7bCB8dE849Bb61C31D8a8841a023918cf01e` | NO |
| `derive_safe_wallet(EOA)` | `0x4D74cB58A37E0224C1a6481A8D9D424f55a2d02e` | YES |
| Polymarket profile API | `0x4D74cB58A37E0224C1a6481A8D9D424f55a2d02e` | — |

This meant:
- **Balances:** The CLI showed $0 because it was querying the proxy-derived address, which has no funds
- **Orders:** Would have been placed against the wrong account (rejected by the CLOB)
- **Deposits:** If we had deposited USDC to the proxy-derived address, the funds would have gone to a contract that Polymarket doesn't manage — effectively lost

---

## What We Tried (The Long Way Around)

### Initial (Incorrect) Diagnosis

The initial analysis concluded that the CLI "never calls `.funder()` on the SDK's authentication builder" and that `create_or_derive_api_key()` "passes `None` for the funder parameter." This led to a plan to fork the CLI and add explicit funder address support.

### The Fork

We forked `Polymarket/polymarket-cli` to `madmansn0w/polymarket-cli` and made the following changes across 6 files (~164 lines added):

**`src/config.rs`** — Added `funder: Option<String>` to the Config struct, a `resolve_funder()` function (with the same priority chain as other config: CLI flag > env var > config file), and `save_wallet_with_funder()`.

**`src/main.rs`** — Added `--funder` as a global CLI flag, threaded it through to the `clob` and `wallet` command dispatchers.

**`src/auth.rs`** — Modified `authenticated_clob_client()` and `authenticate_with_signer()` to accept a `funder_flag` parameter. When set, the auth builder calls `.funder(address)` to override the SDK's auto-derivation.

**`src/commands/clob.rs`** — Updated all 25+ call sites that create authenticated CLOB clients to pass the funder parameter through.

**`src/commands/wallet.rs`** — Added a `Sync` subcommand that queries the Polymarket profile API for the canonical proxy wallet and saves it as the funder in config. Updated `show` to display both the proxy derivation, safe derivation, and configured funder side by side. Made `execute` async to support the API call.

**`src/commands/setup.rs`** — Added a note in the setup wizard suggesting `wallet sync` for website-created accounts.

We also added `POLYMARKET_FUNDER` env var support so crank (our trading engine, which shells out to the `polymarket` binary) could override the funder at the service level without touching the CLI config.

### The Build

The fork was built, installed to `/opt/homebrew/bin/polymarket`, and the `wallet sync` command was run to auto-resolve the funder from the Polymarket profile API. Everything appeared to work — balance queries returned data, the CLOB accepted the auth.

### Where It Went Wrong

During address verification, we discovered the imported private key (`0xb5C3...`) was not the MetaMask EOA (`0xA3f6...`). After re-importing the correct key, the `wallet show` output revealed everything:

```
Address:        0xA3f6A3977Bb58B9Db1E5E5Cb0943eD5C9CfAfF61
Proxy (derive): 0x8d5b7bCB8dE849Bb61C31D8a8841a023918cf01e  ← wrong
Safe (derive):  0x4D74cB58A37E0224C1a6481A8D9D424f55a2d02e  ← correct!
```

The Safe derivation already produced the correct address. The entire funder override was unnecessary — the SDK would have derived the right address automatically if the signature type had been `gnosis-safe` instead of `proxy`.

---

## The Actual Fix

Two commands, zero code changes:

```bash
polymarket wallet import <private-key> --force --signature-type gnosis-safe
polymarket clob create-api-key
```

The CLI and SDK already had complete support for Gnosis Safe wallets. The only mistake was using the default signature type (`proxy`) when the account was created as a Safe.

### Verification

```bash
$ polymarket wallet show -o json
{
  "address": "0xA3f6A3977Bb58B9Db1E5E5Cb0943eD5C9CfAfF61",
  "safe_address": "0x4D74cB58A37E0224C1a6481A8D9D424f55a2d02e",
  "signature_type": "gnosis-safe",
  ...
}

$ polymarket profiles get 0xA3f6A3977Bb58B9Db1E5E5Cb0943eD5C9CfAfF61 -o json
{
  "proxyWallet": "0x4D74cB58A37E0224C1a6481A8D9D424f55a2d02e",
  "name": "0xCog",
  "pseudonym": "Quirky-Parcel",
  ...
}

$ polymarket clob balance --asset-type collateral -o json
{
  "balance": "0",
  "allowances": { ... }
}

$ polymarket clob account-status -o json
{
  "closed_only": false
}
```

All addresses match. Auth works. Balance queries return data for the correct proxy.

---

## What We Kept From the Fork

The fork changes are committed and pushed to `madmansn0w/polymarket-cli`. While the funder override was not the fix for this issue, the following additions have diagnostic and defense-in-depth value:

- **`wallet show`** now displays proxy derivation, safe derivation, and configured funder — this is what revealed the real answer and will prevent future mismatches
- **`wallet sync [ADDRESS]`** auto-resolves the canonical proxy from the Polymarket profile API — useful as a diagnostic and for edge cases
- **`--funder` / `POLYMARKET_FUNDER`** env var provides a manual override for cases where neither derivation matches (migrated accounts, custom deployments)

---

## Lessons Learned

### 1. Validate assumptions before writing code

The plan assumed the CLI was missing funder support. A 30-second diagnostic — running `polymarket profiles get <EOA>` and comparing the result against both `derive_proxy_wallet()` and `derive_safe_wallet()` — would have identified that the SDK already produced the correct address with the right signature type. The entire fork was built on an incorrect diagnosis.

### 2. Understand the domain model

"Proxy wallet" and "Gnosis Safe wallet" are two distinct Polymarket account types with different CREATE2 derivation paths. Polymarket's documentation doesn't make this distinction obvious, and the CLI defaults to `proxy` even though the vast majority of current accounts are Safes. Understanding the difference between these two systems was the key insight.

### 3. Show all possible states, then choose

The breakthrough came from adding both derivations to `wallet show`. Seeing the safe address match the expected proxy immediately pointed to the answer. When debugging address mismatches, the first step should be to enumerate all possible derivations and compare them against the known-correct value.

### 4. Configuration errors don't need code fixes

The signature type flag already existed. The SDK already supported both derivation paths. The CLI already accepted `--signature-type gnosis-safe`. The fix was a configuration value, not a code change. Hours of engineering were spent building infrastructure to work around what was fundamentally a single incorrect default.

### 5. Verify the signer identity early

We initially had the wrong private key imported (`0xb5C3...` instead of `0xA3f6...`). The first wallet sync appeared to work because the funder override bypassed the signer/funder relationship check. Always verify that the configured signer is the EOA that actually controls the target proxy wallet.

---

## Diagnostic Checklist for Future Wallet Issues

Before writing any code, run through these checks:

```bash
# 1. What address does Polymarket think is your proxy?
polymarket profiles get <YOUR_EOA> -o json | jq .proxyWallet

# 2. What does the proxy derivation produce?
polymarket wallet show -o json | jq .proxy_address

# 3. What does the safe derivation produce?
polymarket wallet show -o json | jq .safe_address

# 4. Which one matches?
#    - proxy_address matches  → use --signature-type proxy
#    - safe_address matches   → use --signature-type gnosis-safe
#    - neither matches        → use --funder <address> override

# 5. Is the right key imported?
polymarket wallet show -o json | jq .address
# Should match the EOA you see in MetaMask / your key manager

# 6. After fixing, re-create the API key:
polymarket clob create-api-key

# 7. Verify auth works:
polymarket clob balance --asset-type collateral -o json
```

---

## Production Considerations

For crank (the trading engine that shells out to the `polymarket` binary):

- The production machine needs the same fix: import the key with `--signature-type gnosis-safe`
- The `POLYMARKET_FUNDER` env var in crank's `.env` serves as a safety net but is not required when the signature type is correct
- If the production binary is the stock Homebrew install (without the fork), the signature type fix still works — `--signature-type gnosis-safe` is a feature of the upstream CLI, not our fork
