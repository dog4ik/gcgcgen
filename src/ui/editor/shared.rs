use super::*;

/// Wire formats for a request body, keyed by `Envelope`'s serde name.
pub const ENVELOPE_OPTIONS: &[(&str, &str)] = &[
    ("json", "application/json"),
    ("form", "application/x-www-form-urlencoded"),
];

/// Literal option lists, owned so they can be mixed with computed ones.
pub fn opts(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[component]
pub fn Select(
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
pub fn NumberField(
    #[prop(into)] label: String,
    value: u64,
    on_input: Callback<u64>,
) -> impl IntoView {
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
pub fn NameValueGrid(
    #[prop(into)] label: String,
    rows: Vec<NameValue>,
    rev: RwSignal<u32>,
    on_change: Callback<Vec<NameValue>>,
) -> impl IntoView {
    let state = RwSignal::new(rows);
    // The row count only: reacting to the values would drop the caret.
    let count = Memo::new(move |_| state.get().len());
    let push = move |_| {
        state.update(|r| {
            r.push(NameValue::new(
                "",
                Expr::parse("").expect("empty expression"),
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
                                    placeholder="settings.api_key"
                                    prop:value=row.value.src().to_string()
                                    on:input:target=move |e| {
                                        if let Ok(t) = Expr::parse(e.target().value()) {
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

/// The body's source text, or empty when the request sends none.
pub fn body_json_text(body: &Body) -> String {
    match body {
        Body::Json { expr } => expr.src().to_string(),
        _ => String::new(),
    }
}

/// Parses body text, treating blank as "send no body".
pub fn apply_json_body(text: String, set: impl FnOnce(Body)) -> Option<String> {
    if text.trim().is_empty() {
        set(Body::None);
        return None;
    }
    match Expr::parse(text) {
        Ok(expr) => {
            set(Body::Json { expr });
            None
        }
        Err(e) => Some(e.to_string()),
    }
}
