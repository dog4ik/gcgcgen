use super::*;

#[component]
pub fn RequestRow(
    doc: RwSignal<Integration>,
    rev: RwSignal<u32>,
    part: Part,
    i: usize,
) -> impl IntoView {
    let req = move || doc.with(|d| part.requests(d).and_then(|r| r.get(i).cloned()));
    let set = move |f: Box<dyn FnOnce(&mut RequestDef)>| {
        doc.update(|d| {
            if let Some(x) = part.requests_mut(d).and_then(|r| r.get_mut(i)) {
                f(x)
            }
        })
    };
    // Untracked: this snapshot only seeds the widgets, which own their text
    // from then on.
    let Some(current) = doc.with_untracked(|d| part.requests(d).and_then(|r| r.get(i).cloned()))
    else {
        return ().into_any();
    };
    let auth_options = {
        let mut v = vec![("".to_string(), "— none —".to_string())];
        v.extend(
            doc.get_untracked()
                .auths
                .iter()
                .map(|a| (a.id.0.clone(), a.display().to_string())),
        );
        v
    };

    view! {
        <div class="space-y-3 rounded border border-slate-800 p-3">
            <div class="flex items-center justify-between">
                // Reactive so a rename shows immediately; the widgets below
                // are seeded from the snapshot and must not be.
                <span class="font-mono text-xs text-slate-500">
                    {move || format!("steps.{}", req().map(|r| r.name).unwrap_or_default())}
                </span>
                <Button
                    tone="danger"
                    on_click=Callback::new(move |_| {
                        doc.update(|d| {
                            if let Some(r) = part.requests_mut(d) {
                                r.remove(i);
                            }
                        });
                        rev.update(|r| *r += 1);
                    })
                >
                    "Remove"
                </Button>
            </div>

            <div class="grid gap-3 sm:grid-cols-[1fr_auto_auto_1fr]">
                <TextField
                    label="Name"
                    value=Signal::derive(move || req().map(|r| r.name).unwrap_or_default())
                    on_input=Callback::new(move |v: String| {
                        set(Box::new(move |r| r.name = v))
                    })
                />
                <Select
                    label="Method"
                    options=opts(&[("post", "POST"),
                        ("get", "GET"),
                        ("put", "PUT"),
                        ("patch", "PATCH"),
                        ("delete", "DELETE")])
                    selected=current.method.as_str().to_lowercase()
                    on_change=Callback::new(move |v: String| {
                        if let Ok(m) = serde_json::from_value::<HttpMethod>(serde_json::json!(v)) {
                            set(Box::new(move |r| r.method = m));
                        }
                    })
                />
                <Select
                    label="Envelope"
                    options=opts(ENVELOPE_OPTIONS)
                    selected=current.envelope.as_str()
                    on_change=Callback::new(move |v: String| {
                        if let Ok(e) = serde_json::from_value::<Envelope>(serde_json::json!(v)) {
                            set(Box::new(move |r| r.envelope = e));
                        }
                    })
                />
                <Select
                    label="Auth"
                    options=auth_options
                    selected=current.auth.as_ref().map(|a| a.0.clone()).unwrap_or_default()
                    on_change=Callback::new(move |v: String| {
                        let a = (!v.is_empty()).then(|| AuthId::new(v));
                        set(Box::new(move |r| r.auth = a));
                    })
                />
            </div>

            <ParsedField
                label="Path"
                initial=current.path.src().to_string()
                mono=true
                apply=Callback::new(move |v: String| match Expr::parse(v) {
                    Ok(p) => {
                        set(Box::new(move |r| r.path = p));
                        None
                    }
                    Err(e) => Some(e.to_string()),
                })
            />

            <NameValueGrid
                label="Headers"
                rows=current.headers.clone()
                rev
                on_change=Callback::new(move |rows: Vec<NameValue>| {
                    set(Box::new(move |r| r.headers = rows))
                })
            />

            <ParsedField
                label="Body"
                initial=body_json_text(&current.body)
                mono=true
                rows=10
                hint="One expression, normally an object literal: paste the gateway's example and unquote the values you want computed. \
                      A key whose value is absent is left out. With the form envelope the object is sent urlencoded, nesting as a[b]."
                apply=Callback::new(move |v: String| apply_json_body(v, move |b| {
                    set(Box::new(move |r| r.body = b))
                }))
            />

            <ParsedField
                label="Run if"
                initial=current.run_if.as_ref().map(|e| e.src().to_string()).unwrap_or_default()
                mono=true
                placeholder="always runs"
                hint="Skip this request unless the expression is truthy, e.g. params.phone. Blank always runs it."
                apply=Callback::new(move |v: String| {
                    if v.trim().is_empty() {
                        set(Box::new(|r| r.run_if = None));
                        return None;
                    }
                    match Expr::parse(v) {
                        Ok(e) => {
                            set(Box::new(move |r| r.run_if = Some(e)));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                })
            />

            <ParsedField
                label="Success when"
                initial=current
                    .response
                    .success_when
                    .as_ref()
                    .map(|e| e.src().to_string())
                    .unwrap_or_else(|| "resp.ok".to_string())
                mono=true
                hint="Truthy means success. `resp` carries ok (HTTP 2xx), status, headers and body, \
                      so one expression covers it: resp.ok && resp.body.status == \"Success\". \
                      Blank means resp.ok."
                apply=Callback::new(move |v: String| {
                    if v.trim().is_empty() {
                        set(Box::new(|r| r.response.success_when = None));
                        return None;
                    }
                    match Expr::parse(v) {
                        Ok(e) => {
                            set(Box::new(move |r| r.response.success_when = Some(e)));
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                })
            />

            <div class="grid gap-3 sm:grid-cols-[2fr_1fr]">
                <ParsedField
                    label="Error message"
                    initial=current
                        .response
                        .error
                        .message
                        .as_ref()
                        .map(|e| e.src().to_string())
                        .unwrap_or_default()
                    mono=true
                    hint="Read from the failed response, e.g. resp.body.message"
                    apply=Callback::new(move |v: String| {
                        if v.trim().is_empty() {
                            set(Box::new(|r| r.response.error.message = None));
                            return None;
                        }
                        match Expr::parse(v) {
                            Ok(e) => {
                                set(Box::new(move |r| r.response.error.message = Some(e)));
                                None
                            }
                            Err(e) => Some(e.to_string()),
                        }
                    })
                />
                <Select
                    label="On error"
                    options=opts(&[("fail", "Fail (classify)"),
                        ("pending", "Force pending"),
                        ("declined", "Force declined"),
                        ("continue", "Continue")])
                    selected=serde_json::to_value(current.response.error.on_error)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default()
                    on_change=Callback::new(move |v: String| {
                        if let Ok(o) = serde_json::from_value::<OnError>(serde_json::json!(v)) {
                            set(Box::new(move |r| r.response.error.on_error = o));
                        }
                    })
                />
            </div>
        </div>
    }
    .into_any()
}
