use chemex_cdxml::DrawingDocument;

/// The unique compound groups in a drawing that ChemEx can extract.
pub fn compound_groups(document: &DrawingDocument) -> Vec<String> {
    let mut groups = Vec::new();
    for compound_id in document.fields().iter().map(|field| field.compound_id.as_str()) {
        if groups.iter().any(|group| group == compound_id) {
            continue;
        }
        if document.compound_group(compound_id).is_ok() {
            groups.push(compound_id.to_owned());
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_only_numbered_groups_that_chemex_can_extract() {
        let source = "<CDXML><page id='1'><group id='10'><fragment id='11'><n id='12'/></fragment><t id='13'><s>[[@x]]</s></t></group></page></CDXML>";
        let document = DrawingDocument::parse(source.to_owned()).unwrap();
        let groups = compound_groups(&document);

        assert!(!groups.is_empty());
        assert!(
            groups.iter().all(|id| !id.is_empty() && document.compound_group_source(id).is_ok())
        );
    }
}
