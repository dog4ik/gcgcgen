# gcgcgen

A constructor for payment-gateway integrations. One service hosts many
gateways; each is a **document**, not a code change.

An integration document says how to turn the platform's connect contract into
some gateway's API: what settings arrive, how to authenticate, which requests
each method makes, how their fields map, and how the gateway's answer becomes
`approved | declined | pending`.

## Layout

```
crates/connect/   the reactivepay-facing contract — request/response envelopes,
                  the canonical status vocabulary, the interaction log. No HTTP,
                  no clock, no async: it compiles to wasm and could equally back
                  a hand-written adapter.
src/spec/         the integration document, the expression language, templates,
                  and validation. Pure and wasm-clean, so the editor validates a
                  document as it is typed with no round trip.
src/engine/       execution: rendering, sending, auth and the token cache,
                  logging, and the method executor.
src/api/          POST /gw/{key}/{pay|payout|refund|status}
src/db/           SQLite storage, versioned on every save.
src/ui/           the Leptos editor.
fixtures/         scripay.json — a real integration, and the acceptance test.
examples/         stub_gateway.rs — a fake gateway for local testing.
```

## Running it

```sh
cargo leptos watch                      # http://127.0.0.1:9595
cargo run --example stub_gateway --features ssr   # http://127.0.0.1:8899
```

Seed a document without clicking through the editor:

```sh
cargo run --features ssr -- --import fixtures/scripay.json
```

`DATABASE_URL` (default `sqlite://gcgcgen.db`) and `CALLBACK_BASE` (the public
origin gateways call back on) are the two settings worth setting.

## Field mapping

A request body is JSON whose string leaves may contain `{{ … }}`. Paste the
gateway's example and replace the values:

```json
{
  "order_id": "{{ payment.token }}",
  "amount":   "{{ payment.gateway_amount | minor_to_major }}",
  "channel":  "{{? params.extra_return_param | null_if('_blank_') ?? settings.channel }}",
  "data": {
    "account_name": "{{? [params.first_name, params.last_name] | join(' ') | trim }}"
  }
}
```

Three rules do the work:

- **A lone placeholder keeps its type.** `"{{ … | minor_to_major }}"` renders as
  the number `10`, not `"10"`.
- **`{{? … }}` omits.** An absent result drops the key or array element.
- **Absent means `null` or `""`**, uniformly — for `??`, for `{{? }}`, and for
  the builtins that skip absent inputs.

Scope roots: `payment`, `params`, `settings`, `steps`, `env`, `method`; plus
`resp` inside a response spec and `req` inside a signature's canonical string.
A missing path is `null`, never an error.

## Multi-step methods

A method is an ordered list of requests. Each sees the outputs of the ones
before it under `steps.<name>`, which always carries `ok`, `status`, `headers`
and `body`:

```json
{ "reference": "{{ steps.enquiry.body.reference }}" }
```

## Auth

Defined once per integration and referenced by id, so pay, payout, refund and
status share one cached token. Kinds: none, bearer, basic, header, query,
signature (HMAC/digest over the rendered request), and **token request** — a
real HTTP call whose result is cached.

A token request's `cache_key` must include a merchant-specific setting.
Validation refuses to save one without it, because an empty cache key would let
two merchants on the same integration share a token.

## What the engine decides, not the document

Per-integration configuration is the wrong place to re-litigate these:

1. **Uncertainty is `pending`, never `declined`.** A transport error, a 5xx or
   an unreadable body all mean the gateway may have taken the money.
2. **Every outbound call is logged**, auth calls included — the engine opens the
   span, so a document cannot describe an unlogged request.
3. **Failures are HTTP 200** with `{"result": false, …}`; a non-200 makes the
   platform retry instead of recording the failure.
4. **Secrets are redacted from logs** by value, driven by the settings schema's
   `secret` flags.

Credentials arrive in the `settings` bucket of every request and are never
stored.

## Tests

```sh
cargo test --features ssr
```

No test contacts a real gateway. `tests/scripay_mapping.rs` is the acceptance
test: it renders the Scripay document and asserts the output matches the
hand-written adapter it replaces. `tests/engine_execution.rs` runs the engine
against `wiremock`, mostly to pin down the error matrix.
