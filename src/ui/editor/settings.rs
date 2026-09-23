use super::*;

#[component]
pub fn SettingsTab(doc: RwSignal<Integration>, rev: RwSignal<u32>) -> impl IntoView {
    let add_setting = Callback::new(move |_| {
        doc.update(|d| d.settings.fields.push(FieldDef::new("")));
        rev.update(|r| *r += 1);
    });

    let add_secret = Callback::new(move |_| {
        doc.update(|d| d.redacted_key_list.push(String::new()));
        rev.update(|r| *r += 1);
    });

    view! {
        <Panel
            title="Settings schema"
            subtitle="What reactivepay sends in the `settings` object."
            action=view! { <Button on_click=add_setting>"Add field"</Button> }.into_any()
        >
            {row_list(
                rev,
                move || doc.with_untracked(|d| d.settings.fields.len()),
                move |i| view! { <SettingsRow doc rev i /> },
            )}
        </Panel>
        <Panel
            title="Secret keys"
            subtitle="Integration specific keys that need redaction in logs"
            action=view! { <Button on_click=add_secret>"Add field"</Button> }.into_any()
        >
            {row_list(
                rev,
                move || doc.with_untracked(|d| d.redacted_key_list.len()),
                move |i| view! { <SecretKeyRow doc rev i /> },
            )}
        </Panel>
    }
}

#[component]
pub fn SettingsRow(doc: RwSignal<Integration>, rev: RwSignal<u32>, i: usize) -> impl IntoView {
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
        <div class="grid items-end gap-3 rounded border border-slate-800 p-3 sm:grid-cols-[1fr_auto_auto]">
            <TextField
                label="Key"
                value=Signal::derive(move || field().name)
                on_input=Callback::new(move |v: String| update(Box::new(move |f| f.name = v)))
                placeholder="client_secret"
            />
            <div class="flex flex-col gap-1 pb-1">
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

#[component]
pub fn SecretKeyRow(doc: RwSignal<Integration>, rev: RwSignal<u32>, i: usize) -> impl IntoView {
    // Tracked, for the `Signal::derive` bindings below.
    let field = move || {
        doc.get()
            .redacted_key_list
            .get(i)
            .cloned()
            .unwrap_or_default()
    };
    let update = move |f: Box<dyn FnOnce(&mut String)>| {
        doc.update(|d| {
            if let Some(x) = d.redacted_key_list.get_mut(i) {
                f(x)
            }
        })
    };

    view! {
        <div class="grid items-end gap-3 rounded border border-slate-800 p-3 sm:grid-cols-[1fr_auto_auto]">
            <TextField
                label="Key"
                value=Signal::derive(field)
                on_input=Callback::new(move |v: String| update(Box::new(move |f| *f = v)))
                placeholder="pan"
            />
            <Button
                tone="danger"
                on_click=Callback::new(move |_| {
                    doc.update(|d| {
                        d.redacted_key_list.remove(i);
                    });
                    rev.update(|r| *r += 1);
                })
            >
                "Remove"
            </Button>
        </div>
    }
}
