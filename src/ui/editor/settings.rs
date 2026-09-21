use super::*;

#[component]
pub fn SettingsTab(doc: RwSignal<Integration>, rev: RwSignal<u32>) -> impl IntoView {
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
pub fn SettingsRow(doc: RwSignal<Integration>, rev: RwSignal<u32>, i: usize) -> impl IntoView {
    // Untracked: read once while building this row, which `rev` recreates.
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
