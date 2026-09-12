//! Small shared building blocks for the editor.

use leptos::prelude::*;

/// A text input bound to a value that has to parse — an expression, a
/// template, a JSON body.
///
/// Invalid text stays in the box with the error underneath rather than being
/// reverted, because reverting throws away what the author was in the middle
/// of typing. The document is only updated once the text parses.
///
/// The element is **uncontrolled**: it is seeded once from `initial` and owns
/// its text from then on. Writing the value back on every keystroke would put
/// the caret at the end after each character — assigning `.value` on a
/// `<textarea>` resets the selection in Chrome even when the string has not
/// changed. Nothing upstream needs to push text in either: a structural edit
/// bumps the editor's `rev`, which recreates the whole widget with a fresh
/// `initial`.
#[component]
pub fn ParsedField(
    #[prop(into)] label: String,
    #[prop(into)] initial: String,
    /// Applies the new text. Returns `Some(message)` when it does not parse.
    apply: Callback<String, Option<String>>,
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] hint: String,
    #[prop(optional)] mono: bool,
    #[prop(optional)] rows: usize,
) -> impl IntoView {
    let error = RwSignal::new(None::<String>);
    let on_input = move |value: String| error.set(apply.run(value));

    let base = "w-full rounded border bg-slate-900 px-2 py-1.5 text-sm text-slate-100 \
                placeholder:text-slate-600 focus:outline-none focus:ring-1";
    // Passed as a closure, not called here: the border has to follow `error`.
    let class = move || {
        let tone = if error.get().is_some() {
            "border-rose-600 focus:ring-rose-500"
        } else {
            "border-slate-700 focus:ring-sky-500"
        };
        let font = if mono { " font-mono" } else { "" };
        format!("{base} {tone}{font}")
    };

    let control = if rows > 1 {
        view! {
            <textarea
                class=class
                rows=rows
                placeholder=placeholder
                prop:value=initial
                on:input:target=move |e| on_input(e.target().value())
            />
        }
        .into_any()
    } else {
        view! {
            <input
                class=class
                placeholder=placeholder
                prop:value=initial
                on:input:target=move |e| on_input(e.target().value())
            />
        }
        .into_any()
    };

    view! {
        <label class="block">
            <span class="mb-1 block text-xs font-medium uppercase tracking-wide text-slate-400">
                {label}
            </span>
            {control}
            {move || {
                error
                    .get()
                    .map(|e| {
                        view! { <p class="mt-1 font-mono text-xs text-rose-400">{e}</p> }
                    })
            }}
            {(!hint.is_empty())
                .then(|| view! { <p class="mt-1 text-xs text-slate-500">{hint.clone()}</p> })}
        </label>
    }
}

/// A plain text input bound to a `String` field.
#[component]
pub fn TextField(
    #[prop(into)] label: String,
    #[prop(into)] value: Signal<String>,
    on_input: Callback<String>,
    #[prop(optional, into)] placeholder: String,
    #[prop(optional, into)] hint: String,
) -> impl IntoView {
    view! {
        <label class="block">
            <span class="mb-1 block text-xs font-medium uppercase tracking-wide text-slate-400">
                {label}
            </span>
            <input
                class="w-full rounded border border-slate-700 bg-slate-900 px-2 py-1.5 text-sm \
                       text-slate-100 placeholder:text-slate-600 focus:outline-none \
                       focus:ring-1 focus:ring-sky-500"
                placeholder=placeholder
                prop:value=value
                on:input:target=move |e| on_input.run(e.target().value())
            />
            {(!hint.is_empty()).then(|| view! { <p class="mt-1 text-xs text-slate-500">{hint}</p> })}
        </label>
    }
}

#[component]
pub fn Toggle(
    #[prop(into)] label: String,
    #[prop(into)] value: Signal<bool>,
    on_change: Callback<bool>,
) -> impl IntoView {
    view! {
        <label class="inline-flex cursor-pointer items-center gap-2 text-sm text-slate-300">
            <input
                type="checkbox"
                class="h-4 w-4 rounded border-slate-600 bg-slate-900 accent-sky-500"
                prop:checked=value
                on:change:target=move |e| on_change.run(e.target().checked())
            />
            {label}
        </label>
    }
}

#[component]
pub fn Button(
    children: Children,
    #[prop(optional)] on_click: Option<Callback<()>>,
    #[prop(optional, into)] tone: String,
    #[prop(optional, into)] disabled: Signal<bool>,
) -> impl IntoView {
    let palette = match tone.as_str() {
        "primary" => "bg-sky-600 hover:bg-sky-500 text-white border-sky-500",
        "danger" => "bg-rose-900/40 hover:bg-rose-900/70 text-rose-200 border-rose-800",
        _ => "bg-slate-800 hover:bg-slate-700 text-slate-200 border-slate-700",
    };
    view! {
        <button
            class=format!(
                "rounded border px-3 py-1.5 text-sm font-medium transition \
                 disabled:cursor-not-allowed disabled:opacity-40 {palette}",
            )
            prop:disabled=disabled
            on:click=move |_| {
                if let Some(cb) = on_click {
                    cb.run(())
                }
            }
        >
            {children()}
        </button>
    }
}

/// A titled block with an optional right-hand action.
#[component]
pub fn Panel(
    #[prop(into)] title: String,
    children: Children,
    #[prop(optional, into)] subtitle: String,
    #[prop(optional)] action: Option<AnyView>,
) -> impl IntoView {
    view! {
        <section class="rounded-lg border border-slate-800 bg-slate-900/40">
            <header class="flex items-start justify-between gap-4 border-b border-slate-800 px-4 py-3">
                <div>
                    <h2 class="text-sm font-semibold text-slate-200">{title}</h2>
                    {(!subtitle.is_empty())
                        .then(|| view! { <p class="mt-0.5 text-xs text-slate-500">{subtitle}</p> })}
                </div>
                {action}
            </header>
            <div class="space-y-4 p-4">{children()}</div>
        </section>
    }
}
