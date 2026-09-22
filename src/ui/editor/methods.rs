use super::*;

#[component]
pub fn MethodTab(
    doc: RwSignal<Integration>,
    rev: RwSignal<u32>,
    kind: MethodKind,
) -> impl IntoView {
    // Must not re-run on every edit: it wraps the whole request list.
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
                    Expr::parse("/").expect("static expression"),
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
                        move |i| view! { <RequestRow doc rev part=Part::Method(kind) i /> },
                    )}
                </Panel>
                <ResultForm doc rev part=Part::Method(kind) />
            }
                .into_any()
        }}
    }
}

pub fn default_method(kind: MethodKind) -> MethodDef {
    let name = match kind {
        MethodKind::Status => "status",
        MethodKind::Refund => "refund",
        MethodKind::Payout => "payout",
        MethodKind::Pay => "charge",
    };
    MethodDef {
        requests: vec![RequestDef::new(
            name,
            Expr::parse("/").expect("static expression"),
        )],
        result: ResultMapping {
            status: Expr::parse("\"pending\"").expect("static expression"),
            gateway_token: None,
            amount: None,
            currency: None,
            details: None,
            redirect_request: None,
            requisites: None,
        },
    }
}
