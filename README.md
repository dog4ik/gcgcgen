# gcgcgen

A constructor for payment-gateway integrations. One service hosts many
gateways

## Run

```bash
cargo leptos watch # http://127.0.0.1:64476
```

Seed a document without clicking through the editor:

```bash
cargo run --features ssr -- --import fixtures/scripay.json
```

Settings worth setting:

| Variable | Default | Meaning |
|---|---|---|
| `DATABASE_URL` | `sqlite://gcgcgen.db` | Where documents are stored |
| `CALLBACK_BASE` | the site address | The public origin gateways call back on |
| `BUSINESS_URL` | `http://business:4000` | Where callbacks are forwarded |
| `SIGN_KEY` | unset | The 32-byte key that signs forwarded callbacks. Without it, every forward fails |
| `CALLBACK_CONTEXT_TTL_HOURS` | `24` | How long a pending payment waits for its callback |
| `API_USER` | `admin` | Basic-auth user for the editor |
| `API_PASSWORD` | unset | Basic-auth password. **Without it the editor is open to anyone who can reach it** |

The editor sits behind HTTP basic auth: its pages, the `/api` server functions
that read, write, roll back and dry-run integrations, and the asset bundle. The
browser prompts on the first page load and reuses the credentials from there.
`/gw`, the gateway-facing surface, is unauthenticated: reactivepay and the
gateways call it with no credentials.

## Field mapping

Every authored field is one expression, evaluated against the scope. A request
body is normally an object literal — near-JSON, since keys are string literals
— so the gateway's example still pastes in; unquote the values you want
computed:

```json
"body": { "kind": "json", "expr":
  "{ \"order_id\": payment.token,
     \"amount\": payment.gateway_amount | minor_to_major,
     \"channel\": (params.extra_return_param | null_if('_blank_')) ?? settings.channel,
     \"data\": {
       \"account_name\": [params.first_name, params.last_name] | join(' ') | trim
     } }"
}
```

Four rules do the work:

- **Values keep their type.** `payment.gateway_amount | minor_to_major` is the
  number `10`, not `"10"`.
- **Absence omits.** A missing path is *void*, and a void value drops the key,
  the array element, or the whole field it fills — a `redirect_request` url, a
  header, a query parameter. `void_as_null(…)` is how you keep an explicit
  `null` in a body.
- **Blank is absent**, uniformly: `""` and `null` reaching a builtin come back
  void, so `[a, b] | join(' ') | trim` drops its key when neither name was
  supplied. (`??` is the exception: it falls through on null and void, not on
  `""`.)
- **`|` is the loosest operator.** `a | f ?? b` reads as `a | (f ?? b)`, so a
  fallback after a call needs parentheses.

Text is a quoted literal — `'/v1/gateway/initiate/collection'` — and single
quotes save a round of escaping inside the JSON document. There is no string
interpolation: join text with `+` for two strings, or `concat([…])`, which
stringifies numbers and skips what is absent. Note that a string literal has no
`\n`-style escapes; a canonical signature string that needs a newline carries a
real one.

Scope roots: `payment`, `params`, `settings`, `steps`, `env`, `method`; plus
`resp` inside a response spec and `req` inside a signature's canonical
expression. A missing path is void, never an error — though reading a field off
a string or a number is, so a body that is not the shape the spec expects fails
loudly.

A request's `envelope` picks the wire format of that body: `json` (the default,
`application/json`) or `form` (`application/x-www-form-urlencoded`). A form
body must evaluate to an object; nested values flatten to `a[b]` / `a[0]` keys,
scalars to their plain text, and nulls are dropped. The token request of a
`token_request` auth takes the same field.

Keys come out sorted, not in the order they were written: an object literal is
a hash map. Nothing on the wire depends on body key order — but if a gateway
ever signs a concatenation of the body as sent, check it.

## Multi-step methods

A method is an ordered list of requests. Each sees the outputs of the ones
before it under `steps.<name>`, which always carries `ok`, `status`, `headers`
and `body`:

```json
"expr": "{ \"reference\": steps.enquiry.body.reference }"
```

## The result

Each method ends with a `result` block: the status plus the optional facts the
platform records.

```json
"result": {
  "status": "\"pending\"",
  "gateway_token": "steps.charge.body.rrn",
  "redirect_request": {
    "type": "post",
    "url": "steps.charge.body.pay_url",
    "params": "{ \"order\": payment.token }"
  },
  "requisites": "{ \"account\": steps.charge.body.account }"
}
```

- **`redirect_request`** hands the shopper over to the gateway's own page.
  `type` is one of `post`, `get`, `get_with_processing`, `post_iframes` or
  `redirect_html`. A url that evaluates to nothing drops the whole block, so one
  document serves both a hosted checkout and a straight-through charge.
- **`requisites`** is any object, passed through untouched — a bank account, a
  till number, a reference. Like a request body it is one expression, and a void
  value drops its key.

## Reading a response

A request succeeds when its `success_when` expression is truthy. It sees `resp`
— `ok` (HTTP 2xx), `status`, `headers` and `body` — so one expression says the
whole thing:

```json
"response": {
  "success_when": "resp.ok && resp.body.status == \"Success\"",
  "error": { "message": "resp.body.message", "on_error": "fail" }
}
```
