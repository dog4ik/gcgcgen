//! The integration editor.

mod auth;
mod callback;
mod methods;
mod raw_json;
mod request;
mod result;
mod settings;
mod shared;

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
use crate::spec::settings::FieldDef;
use crate::spec::{
    validate, AckDef, CallbackDef, Envelope, Expr, HttpMethod, IframeDef, Integration, MethodDef,
    RedirectDef, RequestDef, ResultMapping,
};
use crate::ui::widgets::{Button, Panel, ParsedField, TextField, Toggle};

use auth::*;
use callback::*;
use methods::*;
use raw_json::*;
use request::*;
use result::*;
use settings::*;
use shared::*;

/// Renders an indexed list of row widgets, rebuilding **only** when `rev`
/// changes.
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
    Callback,
    Json,
}

/// Which request sequence and result a row or form edits
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Method(MethodKind),
    Callback,
}

impl Part {
    fn requests(self, d: &Integration) -> Option<&Vec<RequestDef>> {
        match self {
            Part::Method(kind) => d.methods.get(&kind).map(|m| &m.requests),
            Part::Callback => d.callback.as_ref().map(|c| &c.requests),
        }
    }

    fn requests_mut(self, d: &mut Integration) -> Option<&mut Vec<RequestDef>> {
        match self {
            Part::Method(kind) => d.methods.get_mut(&kind).map(|m| &mut m.requests),
            Part::Callback => d.callback.as_mut().map(|c| &mut c.requests),
        }
    }

    fn result(self, d: &Integration) -> Option<&ResultMapping> {
        match self {
            Part::Method(kind) => d.methods.get(&kind).map(|m| &m.result),
            Part::Callback => d.callback.as_ref().map(|c| &c.result),
        }
    }

    fn result_mut(self, d: &mut Integration) -> Option<&mut ResultMapping> {
        match self {
            Part::Method(kind) => d.methods.get_mut(&kind).map(|m| &mut m.result),
            Part::Callback => d.callback.as_mut().map(|c| &mut c.result),
        }
    }
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
    // Bumped when a list gains or loses a row, so no row widget keeps text
    // belonging to its old neighbour.
    let rev = RwSignal::new(0u32);
    // Bumped when the JSON tab replaces the document wholesale. Separate from
    // `rev`, which would remount that tab's own textarea mid-keystroke.
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
            status.set(Some(match save_integration(current).await {
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
                    // Tracks `tab` alone: switching tabs remounts, which is
                    // what reseeds the uncontrolled fields.
                    {move || match tab.get() {
                        Tab::Settings => view! { <SettingsTab doc rev /> }.into_any(),
                        Tab::Auth => view! { <AuthTab doc rev /> }.into_any(),
                        Tab::Method(kind) => view! { <MethodTab doc rev kind /> }.into_any(),
                        Tab::Callback => view! { <CallbackTab doc rev /> }.into_any(),
                        Tab::Json => view! { <JsonTab doc reseed /> }.into_any(),
                    }}
                </div>
                <div class="space-y-6">
                    <Issues issues />
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
                                apply=Callback::new(move |v: String| match Expr::parse(v) {
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
                            // A dot: the tab label already names the method.
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
            {move || {
                let defined = doc.get().callback.is_some();
                item(
                    "callback".into(),
                    Tab::Callback,
                    defined
                        .then(|| {
                            view! { <span class="h-1.5 w-1.5 rounded-full bg-emerald-500" /> }
                                .into_any()
                        }),
                )
            }}
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
