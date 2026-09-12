//! The integration editor.
//!
//! The document is held in one `RwSignal<Integration>` and every control edits
//! it structurally, so validation can run on each keystroke — [`validate`] is
//! pure and compiles to wasm, so nothing here needs the server.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use connect::MethodKind;

use crate::server_fns::{load_integration, save_integration};
use crate::spec::auth::{
    AuthDef, AuthId, AuthKind, SigAlg, SigEncoding, SigPlacement, SignatureAuth, TokenPlacement,
    TokenRequestAuth,
};
use crate::spec::request::{Body, NameValue, OnError};
use crate::spec::settings::{FieldDef, FieldType};
use crate::spec::{
    validate, Expr, HttpMethod, Integration, JsonTemplate, MethodDef, RequestDef, ResultMapping,
    Template,
};
use crate::ui::dry_run::DryRun;
use crate::ui::widgets::{Button, Panel, ParsedField, TextField, Toggle};

/// Renders an indexed list of row widgets, rebuilding **only** when `rev`
/// changes.
///
/// Row widgets own their own text state (see [`ParsedField`]), so a rebuild
/// mid-edit would discard whatever is being typed and drop the caret. `rev` is
/// bumped when a list gains, loses or reorders a row — the only times the
/// widgets actually need recreating — and the count is therefore read
/// untracked.
fn row_list<V: IntoView + 'static>(
    rev: RwSignal<u32>,
    count: impl Fn() -> usize + Send + Sync + 'static,
    row: impl Fn(usize) -> V + Send + Sync + 'static,
) -> impl IntoView {
    move || {
        rev.track();
        (0..count()).map(&row).collect_view()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Settings,
    Auth,
    Method(MethodKind),
    Json,
}

#[component]
pub fn Editor() -> impl IntoView {
    let params = use_params_map();
    let key = move || params.read().get("key").unwrap_or_default();
    let doc = Resource::new(key, |key| async move { load_integration(key).await });

    view! {
        <main class="min-h-screen bg-slate-950 text-slate-200">
            <Suspense fallback=|| {
                view! { <p class="p-10 text-sm text-slate-500">"Loading…"</p> }
            }>
                {move || Suspend::new(async move {
                    match doc.await {
                        Ok(initial) => view! { <EditorBody initial /> }.into_any(),
                        Err(e) => {
                            view! {
                                <div class="p-10">
                                    <p class="font-mono text-sm text-rose-400">{e.to_string()}</p>
                                    <A href="/" attr:class="mt-4 inline-block text-sm text-sky-400">
                                        "Back to integrations"
                                    </A>
                                </div>
                            }
                                .into_any()
                        }
                    }
                })}
            </Suspense>
        </main>
    }
}

#[component]
fn EditorBody(initial: Integration) -> impl IntoView {
    let doc = RwSignal::new(initial);
    // Bumped whenever a list gains or loses a row, so every row widget is
    // rebuilt and none keeps text belonging to its old neighbour.
    let rev = RwSignal::new(0u32);
    // Bumped only when the document is replaced wholesale, from the JSON tab.
    // Kept separate from `rev`: the JSON tab lives inside the tab content, so
    // if replacing the document remounted that, its own textarea would be
    // reseeded on every keystroke and the caret would jump to the end.
    let reseed = RwSignal::new(0u32);
    let tab = RwSignal::new(Tab::Method(MethodKind::Pay));
    let status = RwSignal::new(None::<Result<String, String>>);

    let issues = Memo::new(move |_| match validate::validate(&doc.get()) {
        Ok(()) => Vec::new(),
        Err(report) => report.issues,
    });
    let valid = Memo::new(move |_| issues.get().is_empty());

    let save = Action::new(move |_: &()| {
        let current = doc.get_untracked();
        async move {
            status.set(Some(match save_integration(current, None).await {
                Ok(v) => Ok(format!("saved as v{v}")),
                Err(e) => Err(e.to_string()),
            }));
        }
    });

    view! {
        <div class="mx-auto max-w-6xl px-6 py-8">
            <Header doc reseed save valid status />
            <Tabs doc tab />
            <div class="mt-6 grid gap-6 lg:grid-cols-[minmax(0,1fr)_320px]">
                <div class="space-y-6">
                    // Switching tabs remounts, which is what reseeds the
                    // uncontrolled fields after the JSON tab replaces the
                    // document — so this tracks `tab` alone.
                    {move || match tab.get() {
                        Tab::Settings => view! { <SettingsTab doc rev /> }.into_any(),
                        Tab::Auth => view! { <AuthTab doc rev /> }.into_any(),
                        Tab::Method(kind) => view! { <MethodTab doc rev kind /> }.into_any(),
                        Tab::Json => view! { <JsonTab doc reseed /> }.into_any(),
                    }}
                </div>
                <div class="space-y-6">
                    <Issues issues />
                    {move || match tab.get() {
                        Tab::Method(kind) => view! { <DryRun doc kind /> }.into_any(),
                        _ => ().into_any(),
                    }}
                </div>
            </div>
        </div>
    }
}

#[component]
fn Header(
    doc: RwSignal<Integration>,
    reseed: RwSignal<u32>,
    save: Action<(), ()>,
    valid: Memo<bool>,
    status: RwSignal<Option<Result<String, String>>>,
) -> impl IntoView {
    view! {
        <header class="mb-6">
            <A href="/" attr:class="text-xs text-slate-500 hover:text-slate-300">
                "← Integrations"
            </A>
            <div class="mt-2 flex items-start justify-between gap-6">
                <div class="grid flex-1 gap-3 sm:grid-cols-3">
                    <TextField
                        label="Name"
                        value=Signal::derive(move || doc.get().name)
                        on_input=Callback::new(move |v| doc.update(|d| d.name = v))
                    />
                    <TextField
                        label="Key"
                        value=Signal::derive(move || doc.get().key)
                        on_input=Callback::new(move |v| doc.update(|d| d.key = v))
                        hint="URL segment: /gw/{key}/pay"
                    />
                    {move || {
                        reseed.track();
                        view! {
                            <ParsedField
                                label="Base URL"
                                initial=doc.with_untracked(|d| d.base_url.src().to_string())
                                mono=true
                                hint="May branch on settings, e.g. a sandbox switch."
                                apply=Callback::new(move |v: String| match Template::parse(v) {
                                    Ok(t) => {
                                        doc.update(|d| d.base_url = t);
                                        None
                                    }
                                    Err(e) => Some(e.to_string()),
                                })
                            />
                        }
                    }}
                </div>
                <div class="flex shrink-0 flex-col items-end gap-2 pt-5">
                    <Button
                        tone="primary"
                        disabled=Signal::derive(move || !valid.get() || save.pending().get())
                        on_click=Callback::new(move |_| {
                            save.dispatch(());
                        })
                    >
                        {move || if save.pending().get() { "Saving…" } else { "Save" }}
                    </Button>
                    {move || {
                        status
                            .get()
                            .map(|r| match r {
                                Ok(m) => {
                                    view! {
                                        <span class="text-xs text-emerald-400">{m}</span>
                                    }
                                        .into_any()
                                }
                                Err(e) => {
                                    view! {
                                        <span class="max-w-xs text-right font-mono text-xs text-rose-400">
                                            {e}
                                        </span>
                                    }
                                        .into_any()
                                }
                            })
                    }}
                </div>
            </div>
        </header>
    }
}

#[component]
fn Tabs(doc: RwSignal<Integration>, tab: RwSignal<Tab>) -> impl IntoView {
    let item = move |label: String, this: Tab, badge: Option<AnyView>| {
        view! {
            <button
                class=move || {
                    let base = "flex items-center gap-2 border-b-2 px-3 py-2 text-sm transition";
                    if tab.get() == this {
                        format!("{base} border-sky-500 text-slate-100")
                    } else {
                        format!("{base} border-transparent text-slate-500 hover:text-slate-300")
                    }
                }
                on:click=move |_| tab.set(this)
            >
                {label}
                {badge}
            </button>
        }
    };

    view! {
        <nav class="flex flex-wrap gap-1 border-b border-slate-800">
            {item("Settings".into(), Tab::Settings, None)}
            {move || {
                let n = doc.get().auths.len();
                item(
                    "Auth".into(),
                    Tab::Auth,
                    Some(
                        view! { <span class="font-mono text-xs text-slate-600">{n}</span> }
                            .into_any(),
                    ),
                )
            }}
            {MethodKind::ALL
                .into_iter()
                .map(|kind| {
                    view! {
                        {move || {
                            let defined = doc.get().methods.contains_key(&kind);
                            // A dot, not the method's own name again: the tab
                            // label already says which method this is.
                            item(
                                kind.as_str().to_string(),
                                Tab::Method(kind),
                                defined
                                    .then(|| {
                                        view! {
                                            <span class="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                                        }
                                            .into_any()
                                    }),
                            )
                        }}
                    }
                })
                .collect_view()}
            {item("JSON".into(), Tab::Json, None)}
        </nav>
    }
}

#[component]
fn Issues(issues: Memo<Vec<validate::Issue>>) -> impl IntoView {
    view! {
        <Panel title="Validation" subtitle="Runs in the browser on every keystroke.">
            {move || {
                let issues = issues.get();
                if issues.is_empty() {
                    view! {
                        <p class="text-sm text-emerald-400">"No problems found."</p>
                    }
                        .into_any()
                } else {
                    view! {
                        <ul class="space-y-2">
                            {issues
                                .into_iter()
                                .map(|i| {
                                    view! {
                                        <li class="rounded border border-rose-900/60 bg-rose-950/30 px-2 py-1.5">
                                            <p class="font-mono text-[11px] text-rose-500">
                                                {i.location}
                                            </p>
                                            <p class="text-xs text-rose-200">{i.message}</p>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                        .into_any()
                }
            }}
        </Panel>
    }
}

// ---------------------------------------------------------------------------
// settings
// ---------------------------------------------------------------------------

#[component]
fn SettingsTab(doc: RwSignal<Integration>, rev: RwSignal<u32>) -> impl IntoView {
    let add = Callback::new(move |_| {
        doc.update(|d| d.settings.fields.push(FieldDef::new("")));
        rev.update(|r| *r += 1);
    });

    view! {
        <Panel
            title="Settings schema"
            subtitle="What reactivepay sends in the `settings` bucket. Credentials arrive per request and are never stored."
            action=view! { <Button on_click=add>"Add field"</Button> }.into_any()
        >
            {row_list(
                rev,
                move || doc.with_untracked(|d| d.settings.fields.len()),
                move |i| view! { <SettingsRow doc rev i /> },
            )}
        </Panel>
    }
}

#[component]
fn SettingsRow(doc: RwSignal<Integration>, rev: RwSignal<u32>, i: usize) -> impl IntoView {
    // Untracked, for values read once while building this row; the row is
    // recreated when `rev` changes, which is exactly when they go stale.
    let current = doc
        .with_untracked(|d| d.settings.fields.get(i).cloned())
        .unwrap_or_else(|| FieldDef::new(""));
    // Tracked, for the `Signal::derive` bindings below.
    let field = move || {
        doc.get()
            .settings
            .fields
            .get(i)
            .cloned()
            .unwrap_or(FieldDef::new(""))
    };
    let update = move |f: Box<dyn FnOnce(&mut FieldDef)>| {
        doc.update(|d| {
            if let Some(x) = d.settings.fields.get_mut(i) {
                f(x)
            }
        })
    };

    view! {
        <div class="grid items-end gap-3 rounded border border-slate-800 p-3 sm:grid-cols-[1fr_1fr_auto_auto_auto]">
            <TextField
                label="Key"
                value=Signal::derive(move || field().name)
                on_input=Callback::new(move |v: String| update(Box::new(move |f| f.name = v)))
                placeholder="client_secret"
            />
            <TextField
                label="Label"
                value=Signal::derive(move || field().label.unwrap_or_default())
                on_input=Callback::new(move |v: String| {
                    update(Box::new(move |f| f.label = (!v.is_empty()).then_some(v)))
                })
            />
            <label class="block">
                <span class="mb-1 block text-xs uppercase tracking-wide text-slate-400">
                    "Type"
                </span>
                <select
                    class="rounded border border-slate-700 bg-slate-900 px-2 py-1.5 text-sm text-slate-100"
                    on:change:target=move |e| {
                        let v = e.target().value();
                        update(
                            Box::new(move |f| {
                                f.ty = match v.as_str() {
                                    "number" => FieldType::Number,
                                    "bool" => FieldType::Bool,
                                    _ => FieldType::Text,
                                };
                            }),
                        )
                    }
                >
                    {["text", "number", "bool"]
                        .map(|o| {
                            let selected = matches!(
                                (o, current.ty.clone()),
                                ("text", FieldType::Text)
                                    | ("number", FieldType::Number)
                                    | ("bool", FieldType::Bool)
                            );
                            view! {
                                <option value=o selected=selected>
                                    {o}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </label>
            <div class="flex flex-col gap-1 pb-1">
                <Toggle
                    label="Required"
                    value=Signal::derive(move || field().required)
                    on_change=Callback::new(move |v: bool| {
                        update(Box::new(move |f| f.required = v))
                    })
                />
                <Toggle
                    label="Secret"
                    value=Signal::derive(move || field().secret)
                    on_change=Callback::new(move |v: bool| {
                        update(Box::new(move |f| f.secret = v))
                    })
                />
            </div>
            <Button
                tone="danger"
                on_click=Callback::new(move |_| {
                    doc.update(|d| {
                        d.settings.fields.remove(i);
                    });
                    rev.update(|r| *r += 1);
                })
            >
                "Remove"
            </Button>
        </div>
    }
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

#[component]
fn AuthTab(doc: RwSignal<Integration>, rev: RwSignal<u32>) -> impl IntoView {
    let add = Callback::new(move |_| {
        doc.update(|d| {
            let id = AuthId::new(format!("auth{}", d.auths.len() + 1));
            d.auths.push(AuthDef {
                id,
                label: None,
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

const AUTH_KINDS: [(&str, &str); 7] = [
    ("none", "None"),
    ("bearer", "Bearer token"),
    ("basic", "HTTP basic"),
    ("header", "Header"),
    ("query", "Query parameter"),
    ("signature", "Signature"),
    ("token_request", "Token request"),
];

fn kind_tag(k: &AuthKind) -> &'static str {
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

fn default_kind(tag: &str) -> AuthKind {
    let expr = |s: &str| Expr::parse(s).expect("static expression");
    let tpl = |s: &str| Template::parse(s).expect("static template");
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
            value: tpl("{{ settings.api_key }}"),
        },
        "query" => AuthKind::Query {
            name: "api_key".into(),
            value: tpl("{{ settings.api_key }}"),
        },
        "signature" => AuthKind::Signature(Box::new(SignatureAuth {
            canonical: tpl("{{ req.method }}{{ req.path }}{{ req.body_raw }}{{ env.unix_now }}"),
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
                    template: JsonTemplate::parse_str(
                        r#"{"client_id": "{{ settings.client_id }}",
                            "client_secret": "{{ settings.client_secret }}"}"#,
                    )
                    .expect("static template"),
                },
                ..RequestDef::new("auth", tpl("/oauth/token"))
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
fn AuthRow(doc: RwSignal<Integration>, rev: RwSignal<u32>, i: usize) -> impl IntoView {
    // Tracked, for the `Signal::derive` bindings below.
    let def = move || doc.get().auths.get(i).cloned();
    // Untracked, for values read once while building this row. The row is
    // recreated when `rev` changes, which is exactly when they can go stale.
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
                    hint="Referenced by a request's `auth`."
                />
                <TextField
                    label="Label"
                    value=Signal::derive(move || {
                        def().and_then(|d| d.label).unwrap_or_default()
                    })
                    on_input=Callback::new(move |v: String| {
                        update(Box::new(move |a| a.label = (!v.is_empty()).then_some(v)))
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
            // Keyed on `rev`, not on `doc`: the kind-specific form holds its
            // own in-progress text, so it must survive editing.
            {move || {
                rev.track();
                doc.with_untracked(|d| d.auths.get(i).map(|a| a.kind.clone()))
                    .map(|kind| view! { <AuthKindForm doc i kind /> })
            }}
        </div>
    }
}

#[component]
fn AuthKindForm(doc: RwSignal<Integration>, i: usize, kind: AuthKind) -> impl IntoView {
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
                        apply=Callback::new(move |v: String| match Template::parse(v) {
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
fn SignatureForm(doc: RwSignal<Integration>, i: usize, sig: SignatureAuth) -> impl IntoView {
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
                apply=Callback::new(move |v: String| match Template::parse(v) {
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
fn TokenRequestForm(doc: RwSignal<Integration>, i: usize, tr: TokenRequestAuth) -> impl IntoView {
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
            <div class="grid gap-3 sm:grid-cols-[auto_1fr]">
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
                <ParsedField
                    label="Path"
                    initial=tr.request.path.src().to_string()
                    mono=true
                    apply=Callback::new(move |v: String| match Template::parse(v) {
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

// ---------------------------------------------------------------------------
// methods
// ---------------------------------------------------------------------------

#[component]
fn MethodTab(doc: RwSignal<Integration>, rev: RwSignal<u32>, kind: MethodKind) -> impl IntoView {
    // Adding or removing a method bumps `rev`, so this need not — and must
    // not — re-run on every edit: it wraps the whole request list.
    let defined = move || {
        rev.track();
        doc.with_untracked(|d| d.methods.contains_key(&kind))
    };

    let enable = Callback::new(move |_| {
        doc.update(|d| {
            d.methods
                .entry(kind)
                .or_insert_with(|| default_method(kind));
        });
        rev.update(|r| *r += 1);
    });
    let disable = Callback::new(move |_| {
        doc.update(|d| {
            d.methods.remove(&kind);
        });
        rev.update(|r| *r += 1);
    });
    let add_request = Callback::new(move |_| {
        doc.update(|d| {
            if let Some(m) = d.methods.get_mut(&kind) {
                let n = m.requests.len() + 1;
                m.requests.push(RequestDef::new(
                    format!("step{n}"),
                    Template::parse("/").expect("static template"),
                ));
            }
        });
        rev.update(|r| *r += 1);
    });

    view! {
        {move || {
            if !defined() {
                return view! {
                    <Panel title=format!("{kind}") subtitle="Not defined for this integration.">
                        <Button tone="primary" on_click=enable>
                            {format!("Define {kind}")}
                        </Button>
                    </Panel>
                }
                    .into_any();
            }
            view! {
                <Panel
                    title=format!("{kind} requests")
                    subtitle="Run in order. Each one sees `steps.<name>` for every request above it."
                    action=view! {
                        <div class="flex gap-2">
                            <Button on_click=add_request>"Add request"</Button>
                            <Button tone="danger" on_click=disable>
                                "Remove method"
                            </Button>
                        </div>
                    }
                        .into_any()
                >
                    {row_list(
                        rev,
                        move || {
                            doc.with_untracked(|d| {
                                d.methods.get(&kind).map(|m| m.requests.len()).unwrap_or_default()
                            })
                        },
                        move |i| view! { <RequestRow doc rev kind i /> },
                    )}
                </Panel>
                <ResultForm doc kind />
            }
                .into_any()
        }}
    }
}

fn default_method(kind: MethodKind) -> MethodDef {
    let name = match kind {
        MethodKind::Status => "status",
        MethodKind::Refund => "refund",
        MethodKind::Payout => "payout",
        MethodKind::Pay => "charge",
    };
    MethodDef {
        enabled: true,
        requests: vec![RequestDef::new(
            name,
            Template::parse("/").expect("static template"),
        )],
        result: ResultMapping {
            status: Expr::parse("'pending'").expect("static expression"),
            gateway_token: None,
            amount: None,
            currency: None,
            details: None,
        },
    }
}

#[component]
fn RequestRow(
    doc: RwSignal<Integration>,
    rev: RwSignal<u32>,
    kind: MethodKind,
    i: usize,
) -> impl IntoView {
    let req = move || {
        doc.get()
            .methods
            .get(&kind)
            .and_then(|m| m.requests.get(i).cloned())
    };
    let set = move |f: Box<dyn FnOnce(&mut RequestDef)>| {
        doc.update(|d| {
            if let Some(x) = d.methods.get_mut(&kind).and_then(|m| m.requests.get_mut(i)) {
                f(x)
            }
        })
    };
    // Untracked: this snapshot only seeds the widgets, which own their text
    // from then on. The row is recreated when `rev` changes.
    let Some(current) = doc.with_untracked(|d| {
        d.methods
            .get(&kind)
            .and_then(|m| m.requests.get(i).cloned())
    }) else {
        return ().into_any();
    };
    let auth_options = {
        let mut v = vec![("".to_string(), "— none —".to_string())];
        v.extend(
            doc.get_untracked()
                .auths
                .iter()
                .map(|a| (a.id.0.clone(), a.display().to_string())),
        );
        v
    };

    view! {
        <div class="space-y-3 rounded border border-slate-800 p-3">
            <div class="flex items-center justify-between">
                // Reactive so a rename is reflected immediately; the widgets
                // below are seeded from the snapshot and must not be.
                <span class="font-mono text-xs text-slate-500">
                    {move || format!("steps.{}", req().map(|r| r.name).unwrap_or_default())}
                </span>
                <Button
                    tone="danger"
                    on_click=Callback::new(move |_| {
                        doc.update(|d| {
                            if let Some(m) = d.methods.get_mut(&kind) {
                                m.requests.remove(i);
                            }
                        });
                        rev.update(|r| *r += 1);
                    })
                >
                    "Remove"
                </Button>
            </div>

            <div class="grid gap-3 sm:grid-cols-[1fr_auto_2fr_1fr]">
                <TextField
                    label="Name"
                    value=Signal::derive(move || req().map(|r| r.name).unwrap_or_default())
                    on_input=Callback::new(move |v: String| {
                        set(Box::new(move |r| r.name = v))
                    })
                />
                <Select
                    label="Method"
                    options=opts(&[("post", "POST"),
                        ("get", "GET"),
                        ("put", "PUT"),
                        ("patch", "PATCH"),
                        ("delete", "DELETE")])
                    selected=current.method.as_str().to_lowercase()
                    on_change=Callback::new(move |v: String| {
                        if let Ok(m) = serde_json::from_value::<HttpMethod>(serde_json::json!(v)) {
                            set(Box::new(move |r| r.method = m));
                        }
                    })
                />
                <ParsedField
                    label="Path"
                    initial=current.path.src().to_string()
                    mono=true
                    apply=Callback::new(move |v: String| match Template::parse(v) {
                        Ok(p) => {
                            set(Box::new(move |r| r.path = p));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    })
                />
                <Select
                    label="Auth"
                    options=auth_options
                    selected=current.auth.as_ref().map(|a| a.0.clone()).unwrap_or_default()
                    on_change=Callback::new(move |v: String| {
                        let a = (!v.is_empty()).then(|| AuthId::new(v));
                        set(Box::new(move |r| r.auth = a));
                    })
                />
            </div>

            <NameValueGrid
                label="Headers"
                rows=current.headers.clone()
                rev
                on_change=Callback::new(move |rows: Vec<NameValue>| {
                    set(Box::new(move |r| r.headers = rows))
                })
            />

            <ParsedField
                label="JSON body"
                initial=body_json_text(&current.body)
                mono=true
                rows=10
                hint="Paste the gateway's example and replace values with {{ … }}. Use {{? … }} to omit a key when the value is absent."
                apply=Callback::new(move |v: String| apply_json_body(v, move |b| {
                    set(Box::new(move |r| r.body = b))
                }))
            />

            <div class="grid gap-3 sm:grid-cols-[2fr_1fr]">
                <ParsedField
                    label="Error message"
                    initial=current
                        .response
                        .error
                        .message
                        .as_ref()
                        .map(|e| e.src().to_string())
                        .unwrap_or_default()
                    mono=true
                    hint="Read from the failed response, e.g. resp.body.message"
                    apply=Callback::new(move |v: String| {
                        if v.trim().is_empty() {
                            set(Box::new(|r| r.response.error.message = None));
                            return None;
                        }
                        match Expr::parse(v) {
                            Ok(e) => {
                                set(Box::new(move |r| r.response.error.message = Some(e)));
                                None
                            }
                            Err(e) => Some(e.to_string()),
                        }
                    })
                />
                <Select
                    label="On error"
                    options=opts(&[("fail", "Fail (classify)"),
                        ("pending", "Force pending"),
                        ("declined", "Force declined"),
                        ("continue", "Continue")])
                    selected=serde_json::to_value(current.response.error.on_error)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default()
                    on_change=Callback::new(move |v: String| {
                        if let Ok(o) = serde_json::from_value::<OnError>(serde_json::json!(v)) {
                            set(Box::new(move |r| r.response.error.on_error = o));
                        }
                    })
                />
            </div>
        </div>
    }
    .into_any()
}

#[component]
fn ResultForm(doc: RwSignal<Integration>, kind: MethodKind) -> impl IntoView {
    let set = move |f: Box<dyn FnOnce(&mut ResultMapping)>| {
        doc.update(|d| {
            if let Some(m) = d.methods.get_mut(&kind) {
                f(&mut m.result)
            }
        })
    };
    let Some(result) = doc
        .get_untracked()
        .methods
        .get(&kind)
        .map(|m| m.result.clone())
    else {
        return ().into_any();
    };

    let optional = move |label: &'static str,
                         hint: &'static str,
                         initial: String,
                         set_fn: fn(&mut ResultMapping, Option<Expr>)| {
        view! {
            <ParsedField
                label=label
                initial=initial
                mono=true
                hint=hint
                apply=Callback::new(move |v: String| {
                    if v.trim().is_empty() {
                        set(Box::new(move |m| set_fn(m, None)));
                        return None;
                    }
                    match Expr::parse(v) {
                        Ok(e) => {
                            set(Box::new(move |m| set_fn(m, Some(e))));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                })
            />
        }
    };

    view! {
        <Panel
            title="Result"
            subtitle="Built from the final scope and returned to reactivepay."
        >
            <ParsedField
                label="Status"
                initial=result.status.src().to_string()
                mono=true
                hint="Must produce approved, declined or pending. A map table is the usual way."
                apply=Callback::new(move |v: String| match Expr::parse(v) {
                    Ok(e) => {
                        set(Box::new(move |m| m.status = e));
                        None
                    }
                    Err(e) => Some(e.to_string()),
                })
            />
            <div class="grid gap-3 sm:grid-cols-2">
                {optional(
                    "Gateway token",
                    "The gateway's own transaction id.",
                    result.gateway_token.as_ref().map(|e| e.src().to_string()).unwrap_or_default(),
                    |m, e| m.gateway_token = e,
                )}
                {optional(
                    "Amount",
                    "Minor units — pipe through major_to_minor if the gateway sends decimals.",
                    result.amount.as_ref().map(|e| e.src().to_string()).unwrap_or_default(),
                    |m, e| m.amount = e,
                )}
                {optional(
                    "Currency",
                    "",
                    result.currency.as_ref().map(|e| e.src().to_string()).unwrap_or_default(),
                    |m, e| m.currency = e,
                )}
                {optional(
                    "Details",
                    "Free text shown alongside the status.",
                    result.details.as_ref().map(|e| e.src().to_string()).unwrap_or_default(),
                    |m, e| m.details = e,
                )}
            </div>
        </Panel>
    }
    .into_any()
}

// ---------------------------------------------------------------------------
// raw JSON escape hatch
// ---------------------------------------------------------------------------

#[component]
fn JsonTab(doc: RwSignal<Integration>, reseed: RwSignal<u32>) -> impl IntoView {
    let initial = serde_json::to_string_pretty(&doc.get_untracked()).unwrap_or_default();
    view! {
        <Panel
            title="Document"
            subtitle="The stored form. Anything the forms above do not cover is editable here."
        >
            <ParsedField
                label="integration.json"
                initial=initial
                mono=true
                rows=32
                apply=Callback::new(move |v: String| {
                    match serde_json::from_str::<Integration>(&v) {
                        Ok(d) => {
                            doc.set(d);
                            // Reseeds the header's uncontrolled Base URL field.
                            // Deliberately not `rev`: bumping that would
                            // remount this very textarea mid-keystroke.
                            reseed.update(|r| *r += 1);
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                })
            />
        </Panel>
    }
}

// ---------------------------------------------------------------------------
// small shared pieces
// ---------------------------------------------------------------------------

/// Literal option lists, owned so they can be mixed with computed ones.
fn opts(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[component]
fn Select(
    #[prop(into)] label: String,
    options: Vec<(String, String)>,
    #[prop(into)] selected: String,
    on_change: Callback<String>,
) -> impl IntoView {
    view! {
        <label class="block">
            <span class="mb-1 block text-xs uppercase tracking-wide text-slate-400">{label}</span>
            <select
                class="w-full rounded border border-slate-700 bg-slate-900 px-2 py-1.5 text-sm text-slate-100"
                on:change:target=move |e| on_change.run(e.target().value())
            >
                {options
                    .into_iter()
                    .map(|(value, text)| {
                        let is = value == selected;
                        view! {
                            <option value=value selected=is>
                                {text}
                            </option>
                        }
                    })
                    .collect_view()}
            </select>
        </label>
    }
}

#[component]
fn NumberField(#[prop(into)] label: String, value: u64, on_input: Callback<u64>) -> impl IntoView {
    view! {
        <label class="block">
            <span class="mb-1 block text-xs uppercase tracking-wide text-slate-400">{label}</span>
            <input
                type="number"
                min="0"
                class="w-full rounded border border-slate-700 bg-slate-900 px-2 py-1.5 text-sm text-slate-100"
                prop:value=value.to_string()
                on:input:target=move |e| {
                    if let Ok(v) = e.target().value().parse::<u64>() {
                        on_input.run(v)
                    }
                }
            />
        </label>
    }
}

#[component]
fn NameValueGrid(
    #[prop(into)] label: String,
    rows: Vec<NameValue>,
    rev: RwSignal<u32>,
    on_change: Callback<Vec<NameValue>>,
) -> impl IntoView {
    let state = RwSignal::new(rows);
    // Tracks the row count only. Reacting to the values themselves would
    // rebuild the inputs on every keystroke and drop the caret.
    let count = Memo::new(move |_| state.get().len());
    let push = move |_| {
        state.update(|r| {
            r.push(NameValue::new(
                "",
                Template::parse("").expect("empty template"),
            ))
        });
        on_change.run(state.get_untracked());
        rev.update(|r| *r += 1);
    };

    view! {
        <div>
            <div class="mb-1 flex items-center justify-between">
                <span class="text-xs font-medium uppercase tracking-wide text-slate-400">
                    {label}
                </span>
                <button
                    class="text-xs text-sky-400 hover:text-sky-300"
                    on:click=push
                >
                    "+ add"
                </button>
            </div>
            {move || {
                let n = count.get();
                (0..n)
                    .map(|i| {
                        let row = state.with_untracked(|r| r[i].clone());
                        view! {
                            <div class="mb-2 grid gap-2 sm:grid-cols-[1fr_2fr_auto]">
                                <input
                                    class="rounded border border-slate-700 bg-slate-900 px-2 py-1 font-mono text-sm text-slate-100"
                                    placeholder="Header-Name"
                                    prop:value=row.name.clone()
                                    on:input:target=move |e| {
                                        let v = e.target().value();
                                        state
                                            .update(|rows| {
                                                if let Some(r) = rows.get_mut(i) {
                                                    r.name = v
                                                }
                                            });
                                        on_change.run(state.get_untracked());
                                    }
                                />
                                <input
                                    class="rounded border border-slate-700 bg-slate-900 px-2 py-1 font-mono text-sm text-slate-100"
                                    placeholder="{{ settings.api_key }}"
                                    prop:value=row.value.src().to_string()
                                    on:input:target=move |e| {
                                        if let Ok(t) = Template::parse(e.target().value()) {
                                            state
                                                .update(|rows| {
                                                    if let Some(r) = rows.get_mut(i) {
                                                        r.value = t
                                                    }
                                                });
                                            on_change.run(state.get_untracked());
                                        }
                                    }
                                />
                                <button
                                    class="rounded border border-slate-700 px-2 text-xs text-slate-400 hover:text-rose-300"
                                    on:click=move |_| {
                                        state
                                            .update(|rows| {
                                                rows.remove(i);
                                            });
                                        on_change.run(state.get_untracked());
                                        rev.update(|r| *r += 1);
                                    }
                                >
                                    "×"
                                </button>
                            </div>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}

/// The body as pretty JSON text, or empty when the request sends none.
fn body_json_text(body: &Body) -> String {
    match body {
        Body::Json { template } => {
            serde_json::to_string_pretty(&template.to_value()).unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// Parses body text, treating blank as "send no body".
fn apply_json_body(text: String, set: impl FnOnce(Body)) -> Option<String> {
    if text.trim().is_empty() {
        set(Body::None);
        return None;
    }
    match JsonTemplate::parse_str(&text) {
        Ok(template) => {
            set(Body::Json { template });
            None
        }
        Err(e) => Some(e.to_string()),
    }
}
