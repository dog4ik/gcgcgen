use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use crate::ui::{Editor, IntegrationList};

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en" class="bg-slate-950">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <AutoReload options=options.clone() />
                <HydrationScripts options />
                <link rel="stylesheet" id="leptos" href="/pkg/gcgcgen.css" />
                <link rel="shortcut icon" type="image/ico" href="/favicon.ico" />
                <MetaTags />
            </head>
            <body class="bg-slate-950">
                <App />
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Title text="gcgcgen" />
        <Router>
            <Routes fallback=|| {
                view! { <p class="p-10 text-sm text-slate-400">"Page not found."</p> }
            }>
                <Route path=path!("/") view=IntegrationList />
                <Route path=path!("/i/:key") view=Editor />
            </Routes>
        </Router>
    }
}
