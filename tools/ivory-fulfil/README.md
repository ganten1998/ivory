# ivory-fulfil — automated Gumroad fulfilment

A sale arrives, a supporter key goes out. No manual minting, no manual sending.

```
Gumroad sale ──Ping POST──▶ /hook/<secret> ──▶ verify with Gumroad API
                                            └▶ mint (ivory-core encoder)
                                            └▶ append ledger   ← before mail
                                            └▶ email the buyer
```

## Why it is shaped this way

- **Idempotent.** Gumroad retries pings. A `sale_id` already in the ledger
  re-sends the **original** key instead of minting a second one — a buyer with
  two different keys will reasonably assume one is broken.
- **Verified.** Every ping is checked against Gumroad's API before anything is
  minted. Without that, anyone who guesses the URL mints themselves keys.
- **Ledger before mail.** A mail outage must never lose a key someone paid for.
  Anything in the ledger can be re-sent; anything only in a failed email is gone.
- **Same encoder as the app.** It links `ivory-core`, so the minted key is
  produced by exactly the code that verifies it. Re-implementing the encoder in
  another language is the classic way these systems silently break.
- **Self-test at boot.** It mints and verifies once at startup and exits
  non-zero if that fails, so a bad seed is caught immediately rather than on
  someone's purchase.

## Which signing key to use

Use **k2**, and keep **k1 offline** for hand-minted keys. Both public halves
already ship in the app, so a compromised server key can be retired in favour
of k1 without invalidating keys already in customers' hands.

There is no revocation — deliberately, since verification is offline. If the
server key ever leaks, the honest response is to stop signing with it and ship
a release whose keyring drops it, accepting that keys it signed stop working.
At this price point, with no locked features, that is a proportionate risk.

## Configuration

| variable | notes |
|---|---|
| `IVORY_SIGNING_SEED` | hex seed, i.e. the contents of `~/.ivory-signing/k2.seed` |
| `IVORY_HOOK_SECRET` | random path segment — `openssl rand -hex 16` |
| `GUMROAD_TOKEN` | Gumroad access token (Settings → Advanced → Applications) |
| `GUMROAD_SELLER_ID` | optional, cheap first-pass rejection |
| `RESEND_API_KEY` | resend.com API key (any provider works; swap `send_email`) |
| `MAIL_FROM` | e.g. `Ivory <keys@yourdomain>` — must be a verified sender |
| `LEDGER_PATH` | default `/data/ledger.jsonl` — **must be on a persistent volume** |
| `PORT` | default 8080 |

## Deploy (Fly.io shown; any container host works)

```sh
cd <repo root>
fly launch --no-deploy --dockerfile tools/ivory-fulfil/Dockerfile
fly volumes create ivory_data --size 1          # the ledger lives here
fly secrets set \
  IVORY_SIGNING_SEED="$(cat ~/.ivory-signing/k2.seed)" \
  IVORY_HOOK_SECRET="$(openssl rand -hex 16)" \
  GUMROAD_TOKEN=... RESEND_API_KEY=... MAIL_FROM='Ivory <keys@yourdomain>'
fly deploy
```

Mount the volume at `/data` in `fly.toml`, then check the logs for
`signing self-test OK` before pointing Gumroad at it.

## Wire up Gumroad

Gumroad → your product → **Settings → Ping** → set the URL to:

```
https://<your-host>/hook/<IVORY_HOOK_SECRET>
```

## Verifying it end to end

1. `curl https://<host>/health` → `ok`
2. Set the product to pay-what-you-can with a **$0 minimum**, buy your own
   product for $0, and confirm the key arrives and activates in the app.
3. Check the ledger: `fly ssh console -C "tail -1 /data/ledger.jsonl"`.
4. Re-fire the same ping from Gumroad's dashboard — you must receive the
   **same** key, not a new one.

## Re-sending a key

Everything issued is in the ledger. To re-send, grep the buyer's email and
paste the `key` field into a reply — no need to mint a replacement.
