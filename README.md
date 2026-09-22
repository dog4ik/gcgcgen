# gcgcgen

A constructor for payment-gateway integrations. One service hosts many gateways

## Run

```bash
cargo leptos watch # http://127.0.0.1:64476
```

Settings worth setting:

| Variable                     | Default                | Meaning                                                                         |
| ---------------------------- | ---------------------- | ------------------------------------------------------------------------------- |
| `DATABASE_URL`               | `sqlite://gcgcgen.db`  | Database                                                                        |
| `CALLBACK_BASE`              | the site address       | The public origin gateways call back on                                         |
| `BUSINESS_URL`               | `http://business:4000` | Where callbacks are forwarded                                                   |
| `SIGN_KEY`                   | unset                  | The 32-byte key that signs forwarded callbacks. Without it, every forward fails |
| `CALLBACK_CONTEXT_TTL_HOURS` | `24`                   | How long a pending payment waits for its callback                               |
| `API_USER`                   | `admin`                | Basic-auth user for the editor                                                  |
| `API_PASSWORD`               | unset                  | Basic-auth password.                                                            |
| `DISABLE_ANSI`               | false                  | Disable ansi escape sequences in logs                                           |
