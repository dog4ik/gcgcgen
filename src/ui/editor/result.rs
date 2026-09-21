use super::*;

pub const REDIRECT_KINDS: [(&str, &str); 6] = [
    ("", "None"),
    ("post", "Form post"),
    ("get", "GET"),
    ("get_with_processing", "GET with processing"),
    ("post_iframes", "Iframes"),
    ("redirect_html", "Ready-made HTML"),
];

/// The variant a redirect picker switches to. Every destination starts blank,
/// which the engine reads as "no handover", so a half-filled form never sends
/// the shopper to a dead link.
pub fn default_redirect(tag: &str) -> Option<RedirectDef> {
    let blank = || Expr::literal("");
    Some(match tag {
        "post" => RedirectDef::Post {
            url: blank(),
            params: Expr::empty_object(),
        },
        "get" => RedirectDef::Get { url: blank() },
        "get_with_processing" => RedirectDef::GetWithProcessing { url: blank() },
        "post_iframes" => RedirectDef::PostIframes {
            iframes: vec![IframeDef {
                url: blank(),
                data: Expr::empty_object(),
            }],
        },
        "redirect_html" => RedirectDef::RedirectHtml { html: blank() },
        _ => return None,
    })
}

/// The url, or the HTML document for `redirect_html`.
pub fn set_redirect_text(r: &mut RedirectDef, t: Expr) {
    match r {
        RedirectDef::Post { url, .. }
        | RedirectDef::Get { url }
        | RedirectDef::GetWithProcessing { url } => *url = t,
        RedirectDef::RedirectHtml { html } => *html = t,
        RedirectDef::PostIframes { .. } => {}
    }
}

pub fn set_redirect_params(r: &mut RedirectDef, t: Expr) {
    if let RedirectDef::Post { params, .. } = r {
        *params = t
    }
}

#[component]
pub fn ResultForm(doc: RwSignal<Integration>, rev: RwSignal<u32>, part: Part) -> impl IntoView {
    let set = move |f: Box<dyn FnOnce(&mut ResultMapping)>| {
        doc.update(|d| {
            if let Some(m) = part.result_mut(d) {
                f(m)
            }
        })
    };
    let Some(result) = doc.with_untracked(|d| part.result(d).cloned()) else {
        return ().into_any();
    };
    // A callback reports only a verdict.
    let is_callback = part == Part::Callback;

    let set_redirect = move |f: Box<dyn FnOnce(&mut RedirectDef)>| {
        set(Box::new(move |m| {
            if let Some(r) = m.redirect_request.as_mut() {
                f(r)
            }
        }))
    };

    let destination =
        move |label: &'static str, hint: &'static str, rows: usize, initial: String| {
            view! {
                <ParsedField
                    label=label
                    initial=initial
                    mono=true
                    rows=rows
                    hint=hint
                    apply=Callback::new(move |v: String| match Expr::parse(v) {
                        Ok(t) => {
                            set_redirect(Box::new(move |r| set_redirect_text(r, t)));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    })
                />
            }
        };

    const URL_HINT: &str = "Where to send the shopper, e.g.                             steps.charge.body.redirect_url. An absent or blank url drops                             redirect_request from the reply entirely.";

    let redirect_fields = match result.redirect_request.clone() {
        None => ().into_any(),
        Some(RedirectDef::Post { url, params }) => view! {
            {destination("Redirect URL", URL_HINT, 1, url.src().to_string())}
            <ParsedField
                label="Form params"
                initial=params.src().to_string()
                mono=true
                rows=6
                hint="Posted as the form body. One expression over the scope, same rules as a request body."
                apply=Callback::new(move |v: String| {
                    let text = if v.trim().is_empty() { "{}".to_string() } else { v };
                    match Expr::parse(text.clone()) {
                        Ok(t) => {
                            set_redirect(Box::new(move |r| set_redirect_params(r, t)));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                })
            />
        }
            .into_any(),
        Some(RedirectDef::Get { url }) | Some(RedirectDef::GetWithProcessing { url }) => {
            destination("Redirect URL", URL_HINT, 1, url.src().to_string()).into_any()
        }
        Some(RedirectDef::RedirectHtml { html }) => destination(
                "HTML",
                "Rendered as-is. Usually an auto-submitting form the gateway handed back.",
                8,
                html.src().to_string(),
            )
            .into_any(),
        Some(RedirectDef::PostIframes { iframes }) => view! {
            <ParsedField
                label="Iframes (JSON)"
                initial=serde_json::to_string_pretty(&iframes).unwrap_or_default()
                mono=true
                rows=8
                hint="A list of { url, data }. Both are expressions; a frame whose url is absent is dropped."
                apply=Callback::new(move |v: String| {
                    match serde_json::from_str::<Vec<IframeDef>>(&v) {
                        Ok(frames) => {
                            set_redirect(
                                Box::new(move |r| {
                                    if let RedirectDef::PostIframes { iframes } = r {
                                        *iframes = frames
                                    }
                                }),
                            );
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                })
            />
        }
            .into_any(),
    };

    let optional = move |label: &'static str,
                         hint: &'static str,
                         initial: String,
                         set_fn: fn(&mut ResultMapping, Option<Expr>)| {
        view! {
            <ParsedField
                label=label
                initial=initial
                mono=true
                hint=hint
                apply=Callback::new(move |v: String| {
                    if v.trim().is_empty() {
                        set(Box::new(move |m| set_fn(m, None)));
                        return None;
                    }
                    match Expr::parse(v) {
                        Ok(e) => {
                            set(Box::new(move |m| set_fn(m, Some(e))));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                })
            />
        }
    };

    view! {
        <Panel
            title="Result"
            subtitle=if is_callback {
                "Forwarded to reactivepay when approved or declined; pending is not forwarded. \
                 Amount and currency are required, and a decline's details become its reason."
            } else {
                "Built from the final scope and returned to reactivepay."
            }
        >
            <ParsedField
                label="Status"
                initial=result.status.src().to_string()
                mono=true
                hint="Must produce approved, declined or pending. A map table is the usual way."
                apply=Callback::new(move |v: String| match Expr::parse(v) {
                    Ok(e) => {
                        set(Box::new(move |m| m.status = e));
                        None
                    }
                    Err(e) => Some(e.to_string()),
                })
            />
            <div class="grid gap-3 sm:grid-cols-2">
                {(!is_callback)
                    .then(|| optional(
                        "Gateway token",
                        "The gateway's own transaction id.",
                        result.gateway_token.as_ref().map(|e| e.src().to_string()).unwrap_or_default(),
                        |m, e| m.gateway_token = e,
                    ))}
                {optional(
                    "Amount",
                    "Minor units — pipe through major_to_minor if the gateway sends decimals.",
                    result.amount.as_ref().map(|e| e.src().to_string()).unwrap_or_default(),
                    |m, e| m.amount = e,
                )}
                {optional(
                    "Currency",
                    "",
                    result.currency.as_ref().map(|e| e.src().to_string()).unwrap_or_default(),
                    |m, e| m.currency = e,
                )}
                {optional(
                    "Details",
                    "Free text shown alongside the status.",
                    result.details.as_ref().map(|e| e.src().to_string()).unwrap_or_default(),
                    |m, e| m.details = e,
                )}
            </div>

            {(!is_callback).then(|| view! {
            <div class="space-y-3 rounded border border-slate-800 p-3">
                <Select
                    label="Redirect request"
                    options=opts(&REDIRECT_KINDS)
                    selected=result
                        .redirect_request
                        .as_ref()
                        .map(|r| r.tag())
                        .unwrap_or_default()
                        .to_string()
                    on_change=Callback::new(move |v: String| {
                        let r = default_redirect(&v);
                        set(Box::new(move |m| m.redirect_request = r));
                        rev.update(|n| *n += 1);
                    })
                />
                {redirect_fields}
            </div>

            <ParsedField
                label="Requisites"
                initial=result.requisites.as_ref().map(|e| e.src().to_string()).unwrap_or_default()
                mono=true
                rows=6
                hint="Any object, passed through to the platform as-is — pay-in instructions, an \
                      account number, a reference. Blank sends none."
                apply=Callback::new(move |v: String| {
                    if v.trim().is_empty() {
                        set(Box::new(|m| m.requisites = None));
                        return None;
                    }
                    match Expr::parse(v.clone()) {
                        Ok(t) => {
                            set(Box::new(move |m| m.requisites = Some(t)));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                })
            />
            })}
        </Panel>
    }
    .into_any()
}
