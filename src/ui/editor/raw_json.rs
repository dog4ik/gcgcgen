use super::*;

#[component]
pub fn JsonTab(doc: RwSignal<Integration>, reseed: RwSignal<u32>) -> impl IntoView {
    let initial = serde_json::to_string_pretty(&doc.get_untracked()).unwrap_or_default();
    view! {
        <Panel
            title="Document"
            subtitle="The stored form. Anything the forms above do not cover is editable here."
        >
            <ParsedField
                label="integration.json"
                initial=initial
                mono=true
                rows=32
                apply=Callback::new(move |v: String| {
                    match serde_json::from_str::<Integration>(&v) {
                        Ok(d) => {
                            doc.set(d);
                            // Not `rev`: that would remount this very
                            // textarea mid-keystroke.
                            reseed.update(|r| *r += 1);
                            None
                        }
                        Err(e) => Some(e.to_string()),
                    }
                })
            />
        </Panel>
    }
}
