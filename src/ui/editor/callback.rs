use super::*;

#[component]
pub fn CallbackTab(doc: RwSignal<Integration>, rev: RwSignal<u32>) -> impl IntoView {
    let defined = move || {
        rev.track();
        doc.with_untracked(|d| d.callback.is_some())
    };
    let enable = Callback::new(move |_| {
        doc.update(|d| {
            d.callback.get_or_insert_with(default_callback);
        });
        rev.update(|r| *r += 1);
    });
    let disable = Callback::new(move |_| {
        doc.update(|d| d.callback = None);
        rev.update(|r| *r += 1);
    });
    let add_request = Callback::new(move |_| {
        doc.update(|d| {
            if let Some(c) = d.callback.as_mut() {
                let n = c.requests.len() + 1;
                c.requests.push(RequestDef::new(
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
                    <Panel
                        title="callback"
                        subtitle="The gateway reports the outcome later, to env.callback_url."
                    >
                        <Button tone="primary" on_click=enable>
                            "Define callback"
                        </Button>
                    </Panel>
                }
                    .into_any();
            }
            view! {
                <CallbackForm doc disable />
                <Panel
                    title="callback requests"
                    subtitle="Optional. Run in order after the callback is matched, with the \
                              merchant's settings restored — e.g. to re-check the status."
                    action=view! { <Button on_click=add_request>"Add request"</Button> }.into_any()
                >
                    {row_list(
                        rev,
                        move || {
                            doc.with_untracked(|d| {
                                d.callback.as_ref().map(|c| c.requests.len()).unwrap_or_default()
                            })
                        },
                        move |i| view! { <RequestRow doc rev part=Part::Callback i /> },
                    )}
                </Panel>
                <ResultForm doc rev part=Part::Callback />
            }
                .into_any()
        }}
    }
}

pub fn default_callback() -> CallbackDef {
    CallbackDef {
        enabled: true,
        lookup: Expr::parse("callback.body.order_id").expect("static expression"),
        verify: None,
        requests: Vec::new(),
        result: ResultMapping {
            status: Expr::parse("\"pending\"").expect("static expression"),
            gateway_token: None,
            amount: Some(Expr::parse("callback.body.amount").expect("static expression")),
            currency: Some(Expr::parse("callback.body.currency").expect("static expression")),
            details: None,
            redirect_request: None,
            requisites: None,
        },
        ack: AckDef::default(),
    }
}

#[component]
pub fn CallbackForm(doc: RwSignal<Integration>, disable: Callback<()>) -> impl IntoView {
    let set = move |f: Box<dyn FnOnce(&mut CallbackDef)>| {
        doc.update(|d| {
            if let Some(c) = d.callback.as_mut() {
                f(c)
            }
        })
    };
    let Some(current) = doc.with_untracked(|d| d.callback.clone()) else {
        return ().into_any();
    };

    view! {
        <Panel
            title="callback"
            subtitle="Received on env.callback_url. Scope: callback (method, headers, query, body, raw), \
                      settings, steps, env."
            action=view! {
                <Button tone="danger" on_click=disable>
                    "Remove callback"
                </Button>
            }
                .into_any()
        >
            <ParsedField
                label="Lookup"
                initial=current.lookup.src().to_string()
                mono=true
                hint="The id the payment is found by: our payment token or the gateway's id, \
                      whichever it echoes. Sees only callback and env."
                apply=Callback::new(move |v: String| match Expr::parse(v) {
                    Ok(e) => {
                        set(Box::new(move |c| c.lookup = e));
                        None
                    }
                    Err(e) => Some(e.to_string()),
                })
            />
            <ParsedField
                label="Verify"
                initial=current.verify.as_ref().map(|e| e.src().to_string()).unwrap_or_default()
                mono=true
                placeholder="not verified"
                hint="Falsy rejects the callback with a 401, e.g. \
                      callback.headers.signature == (callback.raw | hmac_sha256(settings.secret)). \
                      Blank accepts every callback."
                apply=Callback::new(move |v: String| {
                    if v.trim().is_empty() {
                        set(Box::new(|c| c.verify = None));
                        return None;
                    }
                    match Expr::parse(v) {
                        Ok(e) => {
                            set(Box::new(move |c| c.verify = Some(e)));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                })
            />
            <div class="grid gap-3 sm:grid-cols-[auto_1fr]">
                <NumberField
                    label="Ack status"
                    value=u64::from(current.ack.status)
                    on_input=Callback::new(move |v: u64| {
                        if let Ok(n) = u16::try_from(v) {
                            set(Box::new(move |c| c.ack.status = n));
                        }
                    })
                />
                <ParsedField
                    label="Ack body"
                    initial=current
                        .ack
                        .body
                        .as_ref()
                        .map(|e| e.src().to_string())
                        .unwrap_or_default()
                    mono=true
                    rows=3
                    hint="What the gateway gets back once the callback is handled. Blank sends an empty body."
                    apply=Callback::new(move |v: String| {
                        if v.trim().is_empty() {
                            set(Box::new(|c| c.ack.body = None));
                            return None;
                        }
                        match Expr::parse(v.clone()) {
                            Ok(t) => {
                                set(Box::new(move |c| c.ack.body = Some(t)));
                                None
                            }
                            Err(e) => Some(e.to_string()),
                        }
                    })
                />
            </div>
        </Panel>
    }
    .into_any()
}
