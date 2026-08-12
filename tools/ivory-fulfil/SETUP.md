# Setup, start to finish

Everything below is account work — the code and config are already done.
Roughly 45 minutes, most of it waiting for DNS.

## 1. Buy a domain (~$10–15/yr)

You need one for EMAIL, not vanity: every mail provider requires a verified
sending domain to deliver to strangers, and a "here's what you paid for" email
landing in spam is the worst failure mode there is.

Registrar: **Porkbun** (simple, free WHOIS privacy) or **Cloudflare Registrar**
(sold at cost, no renewal markup — but DNS must live at Cloudflare).

Something like `ivorymidi.com`. Slightly distinctive beats bare "ivory": better
for search, and a little daylight from Synthogy for free.

## 2. Resend (free: 3,000 emails/month)

1. Sign up at resend.com, **Domains → Add Domain**, enter your domain.
2. It prints DNS records — SPF (TXT), DKIM (CNAME or TXT), and a DMARC
   suggestion. Add them at your registrar's DNS panel, verbatim.
3. Wait for "Verified" (minutes to an hour).
4. **API Keys → Create** → copy it. This is `RESEND_API_KEY`.
5. Your sender will be something like `Ivory <keys@ivorymidi.com>` —
   that is `MAIL_FROM`. The mailbox does not need to exist to SEND.

## 3. Gumroad access token

Gumroad → **Settings → Advanced → Applications** → create an application →
generate an access token. That is `GUMROAD_TOKEN`; the service uses it to
verify each sale is real before minting anything.

Note your **seller id** too (visible in the Ping payload, or your profile URL)
for `GUMROAD_SELLER_ID` — a cheap first-pass rejection.

## 4. Deploy (~$2/month)

```sh
brew install flyctl
fly auth signup            # or: fly auth login
cd <repo root>             # IMPORTANT: the Dockerfile needs ivory-core

fly launch --no-deploy --copy-config -c tools/ivory-fulfil/fly.toml
fly volumes create ivory_data --size 1 -c tools/ivory-fulfil/fly.toml

fly secrets set -c tools/ivory-fulfil/fly.toml \
  IVORY_SIGNING_SEED="$(cat ~/.ivory-signing/k2.seed)" \
  IVORY_HOOK_SECRET="<the secret printed for you>" \
  GUMROAD_TOKEN="..." \
  GUMROAD_SELLER_ID="..." \
  RESEND_API_KEY="..." \
  MAIL_FROM="Ivory <keys@ivorymidi.com>"

fly deploy -c tools/ivory-fulfil/fly.toml --dockerfile tools/ivory-fulfil/Dockerfile
```

If `fly launch` claims the app name is taken, change `app =` in fly.toml.

**Confirm before going further:**

```sh
fly logs -c tools/ivory-fulfil/fly.toml     # want: "signing self-test OK (public key 23b041e8...)"
curl https://<your-app>.fly.dev/health      # want: ok
```

That public key must start `23b041e8` — that is k2, and it means the server can
mint keys this build of Ivory will accept. Anything else and stop.

## 5. Point Gumroad at it

Gumroad → your product → **Settings → Ping** → URL:

```
https://<your-app>.fly.dev/hook/<your hook secret>
```

## 6. Test it for real, before anyone else can

1. Create a **100% off discount code** (e.g. `IVORYGIFT`).
2. Buy your own product with it, using a **Gmail address** — Gmail is the
   strictest filter and the likeliest inbox for your buyers.
3. Confirm the key arrives, and **check the spam folder** if it does not.
4. Paste it into Ivory → expect "Thank you, …".
5. `fly ssh console -c tools/ivory-fulfil/fly.toml -C "tail -1 /data/ledger.jsonl"`
6. Re-fire the same ping from Gumroad's dashboard. You must get the **same**
   key back, not a new one. That is the idempotency guarantee doing its job.

Keep that discount code afterwards: it IS your "can't afford it, email me"
path, and it costs you no manual work at all.

## If a key ever needs re-sending

Everything issued is in the ledger; grep the buyer's email and paste the `key`
field into a reply. Never mint a replacement — a customer with two keys will
assume one is broken.
