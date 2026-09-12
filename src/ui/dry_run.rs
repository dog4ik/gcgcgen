//! The dry-run panel: render a request against sample input, send nothing.
//!
//! This is the same code path the golden mapping tests assert against, so what
//! the panel shows is what the gateway would receive.

use leptos::prelude::*;

use connect::{ConnectInput, MethodKind};

use crate::model::PreviewOutput;
use crate::server_fns::preview_request;
use crate::spec::Integration;
use crate::ui::widgets::{Button, Panel};

/// Seeded from the integration itself so the box is useful immediately rather
/// than an empty prompt.
fn sample_for(doc: &Integration, kind: MethodKind) -> String {
    let mut settings = serde_json::Map::new();
    for f in &doc.settings.fields {
        let placeholder = match f.ty {
            crate::spec::FieldType::Bool => serde_json::json!(false),
            crate::spec::FieldType::Number => serde_json::json!(0),
            _ => serde_json::json!(format!("<{}>", f.name)),
        };
        settings.insert(f.name.clone(), f.default.clone().unwrap_or(placeholder));
    }

    let payment = match kind {
        MethodKind::Status | MethodKind::Refund => serde_json::json!({
            "token": "ORD1", "gateway_token": "RRN-1",
            "gateway_amount": 10000, "gateway_currency": "KES"
        }),
        _ => serde_json::json!({
            "token": "ORD1", "gateway_amount": 10000, "gateway_currency": "KES",
            "product": "Invoice", "order_number": "ORD-9"
        }),
    };

    serde_json::to_string_pretty(&serde_json::json!({
        "payment": payment,
        "params": {
            "phone": "254700000000",
            "customer": { "first_name": "John", "last_name": "Doe", "phone": "254700000000" }
        },
        "settings": settings,
    }))
    .unwrap_or_default()
}

#[component]
pub fn DryRun(doc: RwSignal<Integration>, kind: MethodKind) -> impl IntoView {
    let names = Memo::new(move |_| {
        doc.get()
            .methods
            .get(&kind)
            .map(|m| {
                m.requests
                    .iter()
                    .map(|r| r.name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });

    let selected = RwSignal::new(String::new());
    let sample = RwSignal::new(sample_for(&doc.get_untracked(), kind));
    let steps = RwSignal::new("{}".to_string());
    let result = RwSignal::new(None::<Result<PreviewOutput, String>>);

    // Default to the first request, and follow it if it is renamed away.
    Effect::new(move |_| {
        let names = names.get();
        if !names.contains(&selected.get_untracked()) {
            selected.set(names.first().cloned().unwrap_or_default());
        }
    });

    let run = Action::new(move |_: &()| {
        let doc = doc.get_untracked();
        let request = selected.get_untracked();
        let sample = sample.get_untracked();
        let steps_text = steps.get_untracked();
        async move {
            let input: ConnectInput = match serde_json::from_str(&sample) {
                Ok(v) => v,
                Err(e) => {
                    result.set(Some(Err(format!("sample input: {e}"))));
                    return;
                }
            };
            let steps_value: serde_json::Value = match serde_json::from_str(&steps_text) {
                Ok(v) => v,
                Err(e) => {
                    result.set(Some(Err(format!("prior steps: {e}"))));
                    return;
                }
            };
            result.set(Some(
                preview_request(doc, kind, request, input, steps_value)
                    .await
                    .map_err(|e| e.to_string()),
            ));
        }
    });

    let field = "w-full rounded border border-slate-700 bg-slate-950 px-2 py-1.5 \
                 font-mono text-xs text-slate-100 focus:outline-none focus:ring-1 \
                 focus:ring-sky-500";

    view! {
        <Panel title="Dry run" subtitle="Renders the request. Nothing is sent.">
            <label class="block">
                <span class="mb-1 block text-xs uppercase tracking-wide text-slate-400">
                    "Request"
                </span>
                <select
                    class="w-full rounded border border-slate-700 bg-slate-900 px-2 py-1.5 text-sm text-slate-100"
                    on:change:target=move |e| selected.set(e.target().value())
                >
                    {move || {
                        let current = selected.get();
                        names
                            .get()
                            .into_iter()
                            .map(|n| {
                                let is = n == current;
                                let value = n.clone();
                                view! {
                                    <option value=value selected=is>
                                        {n}
                                    </option>
                                }
                            })
                            .collect_view()
                    }}
                </select>
            </label>

            <label class="block">
                <span class="mb-1 block text-xs uppercase tracking-wide text-slate-400">
                    "Sample input"
                </span>
                <textarea
                    class=field
                    rows="14"
                    prop:value=sample
                    on:input:target=move |e| sample.set(e.target().value())
                />
            </label>

            <label class="block">
                <span class="mb-1 block text-xs uppercase tracking-wide text-slate-400">
                    "Prior steps"
                </span>
                <textarea
                    class=field
                    rows="3"
                    prop:value=steps
                    on:input:target=move |e| steps.set(e.target().value())
                />
                <p class="mt-1 text-xs text-slate-500">
                    r#"Stand-ins for earlier requests, e.g. {"auth": {"body": {"access_token": "T"}}}"#
                </p>
            </label>

            <Button
                tone="primary"
                disabled=Signal::derive(move || run.pending().get() || selected.get().is_empty())
                on_click=Callback::new(move |_| {
                    run.dispatch(());
                })
            >
                {move || if run.pending().get() { "Rendering…" } else { "Render" }}
            </Button>

            {move || {
                result
                    .get()
                    .map(|r| match r {
                        Err(e) => {
                            view! {
                                <pre class="overflow-x-auto rounded border border-rose-900 bg-rose-950/30 p-2 font-mono text-xs whitespace-pre-wrap text-rose-300">
                                    {e}
                                </pre>
                            }
                                .into_any()
                        }
                        Ok(out) => {
                            view! {
                                <div class="space-y-2">
                                    <p class="font-mono text-xs break-all text-slate-300">
                                        <span class="text-sky-400">{out.method}</span>
                                        " "
                                        {out.url}
                                    </p>
                                    {(!out.headers.is_empty())
                                        .then(|| {
                                            view! {
                                                <pre class="overflow-x-auto rounded border border-slate-800 bg-slate-950 p-2 font-mono text-xs text-slate-400">
                                                    {out
                                                        .headers
                                                        .iter()
                                                        .map(|(k, v)| format!("{k}: {v}"))
                                                        .collect::<Vec<_>>()
                                                        .join("\n")}
                                                </pre>
                                            }
                                        })}
                                    {out
                                        .body
                                        .as_ref()
                                        .map(|b| {
                                            view! {
                                                <pre class="overflow-x-auto rounded border border-slate-800 bg-slate-950 p-2 font-mono text-xs text-emerald-200">
                                                    {serde_json::to_string_pretty(b)
                                                        .unwrap_or_default()}
                                                </pre>
                                            }
                                        })}
                                </div>
                            }
                                .into_any()
                        }
                    })
            }}
        </Panel>
    }
}
