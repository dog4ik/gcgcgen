use super::*;

#[component]
pub fn AuthTab(doc: RwSignal<Integration>, rev: RwSignal<u32>) -> impl IntoView {
    let add = Callback::new(move |_| {
        doc.update(|d| {
            let id = AuthId::new(format!("auth{}", d.auths.len() + 1));
            d.auths.push(AuthDef {
                id,
                kind: AuthKind::Bearer {
                    token: Expr::parse("settings.api_key").expect("static expression"),
                },
            });
        });
        rev.update(|r| *r += 1);
    });

    view! {
        <Panel
            title="Authentication"
            subtitle="Defined once per integration and referenced by requests, so every method shares one cached token."
            action=view! { <Button on_click=add>"Add auth"</Button> }.into_any()
        >
            {move || {
                rev.track();
                if doc.with_untracked(|d| d.auths.is_empty()) {
                    return view! {
                        <p class="text-sm text-slate-500">
                            "No auth defined. Requests will be sent unauthenticated."
                        </p>
                    }
                        .into_any();
                }
                row_list(
                        rev,
                        move || doc.with_untracked(|d| d.auths.len()),
                        move |i| view! { <AuthRow doc rev i /> },
                    )
                    .into_any()
            }}
        </Panel>
    }
}

pub const AUTH_KINDS: [(&str, &str); 7] = [
    ("none", "None"),
    ("bearer", "Bearer token"),
    ("basic", "HTTP basic"),
    ("header", "Header"),
    ("query", "Query parameter"),
    ("signature", "Signature"),
    ("token_request", "Token request"),
];

pub fn kind_tag(k: &AuthKind) -> &'static str {
    match k {
        AuthKind::None => "none",
        AuthKind::Bearer { .. } => "bearer",
        AuthKind::Basic { .. } => "basic",
        AuthKind::Header { .. } => "header",
        AuthKind::Query { .. } => "query",
        AuthKind::Signature(_) => "signature",
        AuthKind::TokenRequest(_) => "token_request",
    }
}

pub fn default_kind(tag: &str) -> AuthKind {
    let expr = |s: &str| Expr::parse(s).expect("static expression");
    match tag {
        "bearer" => AuthKind::Bearer {
            token: expr("settings.api_key"),
        },
        "basic" => AuthKind::Basic {
            username: expr("settings.username"),
            password: expr("settings.password"),
        },
        "header" => AuthKind::Header {
            name: "X-Api-Key".into(),
            value: expr("settings.api_key"),
        },
        "query" => AuthKind::Query {
            name: "api_key".into(),
            value: expr("settings.api_key"),
        },
        "signature" => AuthKind::Signature(Box::new(SignatureAuth {
            canonical: expr("concat([req.method, req.path, req.body_raw, env.unix_now])"),
            algorithm: SigAlg::HmacSha256,
            secret: Some(expr("settings.api_secret")),
            encoding: SigEncoding::Hex,
            placement: SigPlacement::Header {
                name: "X-Signature".into(),
            },
        })),
        "token_request" => AuthKind::TokenRequest(Box::new(TokenRequestAuth {
            request: RequestDef {
                name: "auth".into(),
                body: Body::Json {
                    expr: expr(
                        r#"{"client_id": settings.client_id,
                            "client_secret": settings.client_secret}"#,
                    ),
                },
                ..RequestDef::new("auth", Expr::literal("/oauth/token"))
            },
            token: expr("resp.body.access_token"),
            expires_in: Some(expr("resp.body.expires_in")),
            default_ttl_secs: 3600,
            refresh_buffer_secs: 300,
            cache_key: vec![expr("settings.client_id")],
            placement: TokenPlacement::Bearer,
        })),
        _ => AuthKind::None,
    }
}

#[component]
pub fn AuthRow(doc: RwSignal<Integration>, rev: RwSignal<u32>, i: usize) -> impl IntoView {
    // Tracked, for the `Signal::derive` bindings below.
    let def = move || doc.get().auths.get(i).cloned();
    // Untracked: read once while building this row, which `rev` recreates.
    let snapshot = doc.with_untracked(|d| d.auths.get(i).cloned());
    let update = move |f: Box<dyn FnOnce(&mut AuthDef)>| {
        doc.update(|d| {
            if let Some(a) = d.auths.get_mut(i) {
                f(a)
            }
        })
    };
    let set_kind = move |k: AuthKind| update(Box::new(move |a| a.kind = k));

    view! {
        <div class="space-y-3 rounded border border-slate-800 p-3">
            <div class="grid items-end gap-3 sm:grid-cols-[1fr_1fr_1fr_auto]">
                <TextField
                    label="Id"
                    value=Signal::derive(move || {
                        def().map(|d| d.id.0).unwrap_or_default()
                    })
                    on_input=Callback::new(move |v: String| {
                        update(Box::new(move |a| a.id = AuthId::new(v)))
                    })
                />
                <label class="block">
                    <span class="mb-1 block text-xs uppercase tracking-wide text-slate-400">
                        "Kind"
                    </span>
                    <select
                        class="w-full rounded border border-slate-700 bg-slate-900 px-2 py-1.5 text-sm text-slate-100"
                        on:change:target=move |e| {
                            set_kind(default_kind(&e.target().value()));
                            rev.update(|r| *r += 1);
                        }
                    >
                        {AUTH_KINDS
                            .map(|(tag, label)| {
                                let selected = snapshot
                                    .as_ref()
                                    .is_some_and(|d| kind_tag(&d.kind) == tag);
                                view! {
                                    <option value=tag selected=selected>
                                        {label}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                </label>
                <Button
                    tone="danger"
                    on_click=Callback::new(move |_| {
                        doc.update(|d| {
                            d.auths.remove(i);
                        });
                        rev.update(|r| *r += 1);
                    })
                >
                    "Remove"
                </Button>
            </div>
            // Keyed on `rev`, not `doc`: the form holds in-progress text.
            {move || {
                rev.track();
                doc.with_untracked(|d| d.auths.get(i).map(|a| a.kind.clone()))
                    .map(|kind| view! { <AuthKindForm doc i kind /> })
            }}
        </div>
    }
}

#[component]
pub fn AuthKindForm(doc: RwSignal<Integration>, i: usize, kind: AuthKind) -> impl IntoView {
    let set = move |f: Box<dyn FnOnce(&mut AuthKind)>| {
        doc.update(|d| {
            if let Some(a) = d.auths.get_mut(i) {
                f(&mut a.kind)
            }
        })
    };

    let expr_field = move |label: &'static str,
                           initial: String,
                           set_fn: fn(&mut AuthKind, Expr),
                           hint: &'static str| {
        view! {
            <ParsedField
                label=label
                initial=initial
                mono=true
                hint=hint
                apply=Callback::new(move |v: String| match Expr::parse(v) {
                    Ok(e) => {
                        set(Box::new(move |k| set_fn(k, e)));
                        None
                    }
                    Err(err) => Some(err.to_string()),
                })
            />
        }
    };

    match kind {
        AuthKind::None => ().into_any(),

        AuthKind::Bearer { token } => expr_field(
            "Token expression",
            token.src().into(),
            |k, e| {
                if let AuthKind::Bearer { token } = k {
                    *token = e
                }
            },
            "Sent as `Authorization: Bearer …`.",
        )
        .into_any(),

        AuthKind::Basic { username, password } => view! {
            <div class="grid gap-3 sm:grid-cols-2">
                {expr_field(
                    "Username",
                    username.src().into(),
                    |k, e| {
                        if let AuthKind::Basic { username, .. } = k {
                            *username = e
                        }
                    },
                    "",
                )}
                {expr_field(
                    "Password",
                    password.src().into(),
                    |k, e| {
                        if let AuthKind::Basic { password, .. } = k {
                            *password = e
                        }
                    },
                    "",
                )}
            </div>
        }
        .into_any(),

        AuthKind::Header { name, value } | AuthKind::Query { name, value } => {
            let is_header = doc.with_untracked(|d| {
                matches!(
                    d.auths.get(i).map(|a| &a.kind),
                    Some(AuthKind::Header { .. })
                )
            });
            view! {
                <div class="grid gap-3 sm:grid-cols-2">
                    <TextField
                        label=if is_header { "Header name" } else { "Parameter name" }
                        value=Signal::derive(move || name.clone())
                        on_input=Callback::new(move |v: String| {
                            set(
                                Box::new(move |k| {
                                    match k {
                                        AuthKind::Header { name, .. }
                                        | AuthKind::Query { name, .. } => *name = v,
                                        _ => {}
                                    }
                                }),
                            )
                        })
                    />
                    <ParsedField
                        label="Value"
                        initial=value.src().to_string()
                        mono=true
                        apply=Callback::new(move |v: String| match Expr::parse(v) {
                            Ok(t) => {
                                set(
                                    Box::new(move |k| {
                                        match k {
                                            AuthKind::Header { value, .. }
                                            | AuthKind::Query { value, .. } => *value = t,
                                            _ => {}
                                        }
                                    }),
                                );
                                None
                            }
                            Err(e) => Some(e.to_string()),
                        })
                    />
                </div>
            }
            .into_any()
        }

        AuthKind::Signature(sig) => view! { <SignatureForm doc i sig=*sig /> }.into_any(),

        AuthKind::TokenRequest(tr) => view! { <TokenRequestForm doc i tr=*tr /> }.into_any(),
    }
}

#[component]
pub fn SignatureForm(doc: RwSignal<Integration>, i: usize, sig: SignatureAuth) -> impl IntoView {
    let set = move |f: Box<dyn FnOnce(&mut SignatureAuth)>| {
        doc.update(|d| {
            if let Some(AuthKind::Signature(s)) = d.auths.get_mut(i).map(|a| &mut a.kind) {
                f(s)
            }
        })
    };
    let (name, is_query) = match &sig.placement {
        SigPlacement::Header { name } => (name.clone(), false),
        SigPlacement::Query { name } => (name.clone(), true),
        SigPlacement::BodyField { path } => (path.clone(), false),
    };

    view! {
        <div class="space-y-3">
            <ParsedField
                label="Canonical string"
                initial=sig.canonical.src().to_string()
                mono=true
                rows=2
                hint="Signed after the request is rendered: `req.method`, `req.path`, `req.url`, `req.body_raw`."
                apply=Callback::new(move |v: String| match Expr::parse(v) {
                    Ok(t) => {
                        set(Box::new(move |s| s.canonical = t));
                        None
                    }
                    Err(e) => Some(e.to_string()),
                })
            />
            <div class="grid gap-3 sm:grid-cols-3">
                <Select
                    label="Algorithm"
                    options=opts(&[("hmac_sha256", "HMAC-SHA-256"),
                        ("hmac_sha512", "HMAC-SHA-512"),
                        ("sha256", "SHA-256"),
                        ("sha512", "SHA-512"),
                        ("md5", "MD5")])
                    selected=serde_json::to_value(sig.algorithm)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default()
                    on_change=Callback::new(move |v: String| {
                        if let Ok(a) = serde_json::from_value::<SigAlg>(serde_json::json!(v)) {
                            set(Box::new(move |s| s.algorithm = a));
                        }
                    })
                />
                <Select
                    label="Encoding"
                    options=opts(&[("hex", "Hex"), ("base64", "Base64"), ("base64_url", "Base64 URL")])
                    selected=serde_json::to_value(sig.encoding)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default()
                    on_change=Callback::new(move |v: String| {
                        if let Ok(e) = serde_json::from_value::<SigEncoding>(serde_json::json!(v)) {
                            set(Box::new(move |s| s.encoding = e));
                        }
                    })
                />
                <TextField
                    label=if is_query { "Query parameter" } else { "Header name" }
                    value=Signal::derive(move || name.clone())
                    on_input=Callback::new(move |v: String| {
                        set(
                            Box::new(move |s| {
                                s.placement = match &s.placement {
                                    SigPlacement::Query { .. } => SigPlacement::Query { name: v },
                                    SigPlacement::BodyField { .. } => {
                                        SigPlacement::BodyField { path: v }
                                    }
                                    SigPlacement::Header { .. } => SigPlacement::Header { name: v },
                                };
                            }),
                        )
                    })
                />
            </div>
            <ParsedField
                label="Secret"
                initial=sig.secret.as_ref().map(|e| e.src().to_string()).unwrap_or_default()
                mono=true
                hint="Required by the HMAC algorithms."
                apply=Callback::new(move |v: String| {
                    if v.trim().is_empty() {
                        set(Box::new(|s| s.secret = None));
                        return None;
                    }
                    match Expr::parse(v) {
                        Ok(e) => {
                            set(Box::new(move |s| s.secret = Some(e)));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                })
            />
        </div>
    }
}

#[component]
pub fn TokenRequestForm(
    doc: RwSignal<Integration>,
    i: usize,
    tr: TokenRequestAuth,
) -> impl IntoView {
    let set = move |f: Box<dyn FnOnce(&mut TokenRequestAuth)>| {
        doc.update(|d| {
            if let Some(AuthKind::TokenRequest(t)) = d.auths.get_mut(i).map(|a| &mut a.kind) {
                f(t)
            }
        })
    };
    let body_text = body_json_text(&tr.request.body);
    let cache_key_text = tr
        .cache_key
        .iter()
        .map(|e| e.src().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    view! {
        <div class="space-y-3 rounded border border-slate-800/80 bg-slate-950/40 p-3">
            <p class="text-xs text-slate-500">
                "Runs a real request to fetch a token, then caches it. It gets its own interaction log entry."
            </p>
            <div class="grid gap-3 sm:grid-cols-[auto_auto_1fr]">
                <Select
                    label="Method"
                    options=opts(&[("post", "POST"), ("get", "GET"), ("put", "PUT")])
                    selected=tr.request.method.as_str().to_lowercase()
                    on_change=Callback::new(move |v: String| {
                        if let Ok(m) = serde_json::from_value::<HttpMethod>(serde_json::json!(v)) {
                            set(Box::new(move |t| t.request.method = m));
                        }
                    })
                />
                <Select
                    label="Envelope"
                    options=opts(ENVELOPE_OPTIONS)
                    selected=tr.request.envelope.as_str()
                    on_change=Callback::new(move |v: String| {
                        if let Ok(e) = serde_json::from_value::<Envelope>(serde_json::json!(v)) {
                            set(Box::new(move |t| t.request.envelope = e));
                        }
                    })
                />
                <ParsedField
                    label="Path"
                    initial=tr.request.path.src().to_string()
                    mono=true
                    apply=Callback::new(move |v: String| match Expr::parse(v) {
                        Ok(p) => {
                            set(Box::new(move |t| t.request.path = p));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    })
                />
            </div>
            <ParsedField
                label="Request body"
                initial=body_text
                mono=true
                rows=5
                apply=Callback::new(move |v: String| apply_json_body(v, move |b| {
                    set(Box::new(move |t| t.request.body = b))
                }))
            />
            <div class="grid gap-3 sm:grid-cols-2">
                <ParsedField
                    label="Token expression"
                    initial=tr.token.src().to_string()
                    mono=true
                    hint="Read from the auth response, e.g. resp.body.access_token"
                    apply=Callback::new(move |v: String| match Expr::parse(v) {
                        Ok(e) => {
                            set(Box::new(move |t| t.token = e));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    })
                />
                <ParsedField
                    label="Expires in (seconds)"
                    initial=tr.expires_in.as_ref().map(|e| e.src().to_string()).unwrap_or_default()
                    mono=true
                    hint="Blank falls back to the default TTL."
                    apply=Callback::new(move |v: String| {
                        if v.trim().is_empty() {
                            set(Box::new(|t| t.expires_in = None));
                            return None;
                        }
                        match Expr::parse(v) {
                            Ok(e) => {
                                set(Box::new(move |t| t.expires_in = Some(e)));
                                None
                            }
                            Err(e) => Some(e.to_string()),
                        }
                    })
                />
            </div>
            <ParsedField
                label="Cache key"
                initial=cache_key_text
                mono=true
                hint="Comma-separated. Must include a merchant-specific setting, or every merchant on this integration shares one token."
                apply=Callback::new(move |v: String| {
                    let mut parsed = Vec::new();
                    for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                        match Expr::parse(part) {
                            Ok(e) => parsed.push(e),
                            Err(e) => return Some(e.to_string()),
                        }
                    }
                    set(Box::new(move |t| t.cache_key = parsed));
                    None
                })
            />
            <div class="grid gap-3 sm:grid-cols-3">
                <NumberField
                    label="Default TTL (s)"
                    value=tr.default_ttl_secs
                    on_input=Callback::new(move |v: u64| {
                        set(Box::new(move |t| t.default_ttl_secs = v))
                    })
                />
                <NumberField
                    label="Refresh buffer (s)"
                    value=tr.refresh_buffer_secs
                    on_input=Callback::new(move |v: u64| {
                        set(Box::new(move |t| t.refresh_buffer_secs = v))
                    })
                />
                <Select
                    label="Placement"
                    options=opts(&[("bearer", "Authorization: Bearer"), ("header", "Header"), ("query", "Query")])
                    selected=match tr.placement {
                        TokenPlacement::Bearer => "bearer".into(),
                        TokenPlacement::Header { .. } => "header".to_string(),
                        TokenPlacement::Query { .. } => "query".to_string(),
                    }
                    on_change=Callback::new(move |v: String| {
                        let p = match v.as_str() {
                            "header" => {
                                TokenPlacement::Header { name: "X-Token".into(), prefix: None }
                            }
                            "query" => TokenPlacement::Query { name: "access_token".into() },
                            _ => TokenPlacement::Bearer,
                        };
                        set(Box::new(move |t| t.placement = p));
                    })
                />
            </div>
        </div>
    }
}
