use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_navigate;

use connect::MethodKind;

use crate::server_fns::{delete_integration, list_integrations, save_integration};
use crate::spec::Integration;
use crate::ui::editor::Part;
use crate::ui::widgets::Button;

#[component]
pub fn IntegrationList() -> impl IntoView {
    let reload = RwSignal::new(0u32);
    let integrations = Resource::new(
        move || reload.get(),
        |_| async move { list_integrations().await },
    );
    let error = RwSignal::new(None::<String>);

    let create = Action::new(move |_: &()| async move {
        match save_integration(starter()).await {
            Ok(_) => Some(starter().key),
            Err(e) => {
                error.set(Some(e.to_string()));
                None
            }
        }
    });

    // Jump straight into the editor once the blank document is stored.
    Effect::new(move |_| {
        if let Some(Some(key)) = create.value().get() {
            reload.update(|n| *n += 1);
            use_navigate()(&format!("/i/{key}"), Default::default());
        }
    });

    let remove = Action::new(move |key: &String| {
        let key = key.clone();
        async move {
            if let Err(e) = delete_integration(key).await {
                error.set(Some(e.to_string()));
            }
            reload.update(|n| *n += 1);
        }
    });

    view! {
        <main class="min-h-screen bg-slate-950 text-slate-200">
            <div class="mx-auto max-w-5xl px-6 py-10">
                <header class="mb-8 flex items-end justify-between">
                    <div>
                        <h1 class="text-xl font-semibold text-slate-100">"Integrations"</h1>
                        <p class="mt-1 text-sm text-slate-500">
                            "Each one maps the connect contract onto a gateway's API."
                        </p>
                    </div>
                    <Button
                        tone="primary"
                        on_click=Callback::new(move |_| {
                            create.dispatch(());
                        })
                    >
                        "New integration"
                    </Button>
                </header>

                {move || {
                    error
                        .get()
                        .map(|e| {
                            view! {
                                <p class="mb-4 rounded border border-rose-800 bg-rose-950/50 px-3 py-2 font-mono text-xs text-rose-300">
                                    {e}
                                </p>
                            }
                        })
                }}

                <Suspense fallback=|| {
                    view! { <p class="text-sm text-slate-500">"Loading…"</p> }
                }>
                    {move || Suspend::new(async move {
                        match integrations.await {
                            Err(e) => {
                                view! {
                                    <p class="font-mono text-xs text-rose-400">{e.to_string()}</p>
                                }
                                    .into_any()
                            }
                            Ok(rows) if rows.is_empty() => {
                                view! {
                                    <div class="rounded-lg border border-dashed border-slate-800 px-6 py-12 text-center">
                                        <p class="text-sm text-slate-400">"No integrations yet."</p>
                                        <p class="mt-1 text-xs text-slate-600">
                                            "A new one starts with a pay method and one request."
                                        </p>
                                    </div>
                                }
                                    .into_any()
                            }
                            Ok(rows) => {
                                view! {
                                    <ul class="divide-y divide-slate-800 rounded-lg border border-slate-800">
                                        <For each=move || rows.clone() key=|r| r.key.clone() let:row>
                                            {
                                                let key = row.key.clone();
                                                view! {
                                            <li class="flex items-center gap-4 px-4 py-3 hover:bg-slate-900/50">
                                                <A
                                                    href=format!("/i/{}", row.key)
                                                    attr:class="flex-1 min-w-0"
                                                >
                                                    <span class="block truncate text-sm font-medium text-slate-100">
                                                        {row.name.clone()}
                                                    </span>
                                                    <span class="block truncate font-mono text-xs text-slate-500">
                                                        {row.key.clone()}
                                                    </span>
                                                </A>
                                                <div class="flex shrink-0 gap-1">
                                                    <For
                                                        each={
                                                            let m = row.methods.clone();
                                                            move || m.clone()
                                                        }
                                                        key=|m| *m
                                                        let:m
                                                    >
                                                        <MethodChip kind=Part::Method(m) />
                                                    </For>
                                                    <Show
                                                        when={
                                                            move || row.callback_defined
                                                        }
                                                    >
                                                        <MethodChip kind=Part::Callback />
                                                    </Show>
                                                </div>
                                                <span class="w-20 shrink-0 text-right font-mono text-xs text-slate-600">
                                                    {format!("v{}", row.version)}
                                                </span>
                                                <Button
                                                    tone="danger"
                                                    on_click=Callback::new(move |_| {
                                                        remove.dispatch(key.clone());
                                                    })
                                                >
                                                    "Delete"
                                                </Button>
                                            </li>
                                                }
                                            }
                                        </For>
                                    </ul>
                                }
                                    .into_any()
                            }
                        }
                    })}
                </Suspense>
            </div>
        </main>
    }
}

#[component]
pub fn MethodChip(kind: Part) -> impl IntoView {
    let tone = match kind {
        Part::Method(MethodKind::Pay) => "bg-emerald-950 text-emerald-300 border-emerald-900",
        Part::Method(MethodKind::Payout) => "bg-amber-950 text-amber-300 border-amber-900",
        Part::Method(MethodKind::Refund) => "bg-violet-950 text-violet-300 border-violet-900",
        Part::Method(MethodKind::Status) => "bg-sky-950 text-sky-300 border-sky-900",
        Part::Callback => "bg-red-950 text-red-300 border-red-900",
    };
    view! {
        <span class=format!(
            "rounded border px-1.5 py-0.5 font-mono text-[10px] uppercase {tone}",
        )>{kind.as_str()}</span>
    }
}

/// A minimal but valid document
pub fn starter() -> Integration {
    let key = "new-integration";
    serde_json::from_value(serde_json::json!({
        "key": key,
        "name": "New integration",
        "base_url": "'https://api.example.com'",
        "settings": { "fields": [
            { "name": "api_key", "secret": true }
        ] },
        "auths": [],
        "methods": { "pay": {
            "requests": [{
                "name": "pay",
                "method": "post",
                "path": "'/v1/pay'",
                "body": { "kind": "json", "expr":
                    "{\"reference\": payment.token,
                      \"amount\": payment.gateway_amount | minor_to_major,
                      \"currency\": payment.gateway_currency}" },
                "response": { "error": { "message": "resp.body.message" } }
            }],
            "result": {
                "status": "\"pending\"",
                "gateway_token": "steps.pay.body.id"
            }
        } }
    }))
    .expect("the starter document is well formed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_starter_document_is_valid() {
        crate::spec::validate::validate(&starter()).expect("a new integration must open clean");
    }
}
