use quick_xml::escape::resolve_xml_entity;
use quick_xml::events::Event;

/// Append XML character data. The reader emits references separately from text.
pub(crate) fn append(output: &mut String, event: &Event<'_>) {
    match event {
        Event::Text(text) => {
            if let Ok(text) = text.decode() {
                output.push_str(&text);
            }
        }
        Event::CData(text) => {
            if let Ok(text) = text.decode() {
                output.push_str(&text);
            }
        }
        Event::GeneralRef(reference) => {
            if let Ok(Some(character)) = reference.resolve_char_ref() {
                output.push(character);
            } else if let Ok(name) = reference.decode() {
                if let Some(text) = resolve_xml_entity(&name) {
                    output.push_str(text);
                } else {
                    // Preserve unknown references without expanding a document's DTD.
                    output.push('&');
                    output.push_str(&name);
                    output.push(';');
                }
            }
        }
        _ => {}
    }
}
