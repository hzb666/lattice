use chemex_cdxml::DrawingDocument;

fn drawing(content: &str) -> DrawingDocument {
    DrawingDocument::parse(format!("<CDXML><page>{content}</page></CDXML>")).unwrap()
}

fn fragment_ids(document: &DrawingDocument, field: usize) -> Vec<&str> {
    document.fields()[field]
        .fragment_objects
        .iter()
        .map(|index| document.objects()[*index].id.as_deref().unwrap())
        .collect()
}

#[test]
fn a_group_links_a_label_to_its_authored_fragments_including_salt_components() {
    let document = drawing(
        "<group id='10'><fragment id='11'><n id='12'/></fragment><group><fragment id='13'><n id='14'/></fragment></group><t id='15'><s>[[@x]]</s></t></group>",
    );
    let group = document.fields()[0].group_object.unwrap();
    assert_eq!(Some("10"), document.objects()[group].id.as_deref());
    assert_eq!(vec!["11", "13"], fragment_ids(&document, 0));
}

#[test]
fn inner_compound_groups_do_not_bind_to_other_structures_in_an_outer_scheme_group() {
    let document = drawing(
        "<group id='10'><group id='20'><fragment id='21'/><t><s>[[@x]]</s></t></group><group id='30'><fragment id='31'/><t><s>[[@y]]</s></t></group><fragment id='40'/></group>",
    );
    for (field, expected_group, expected_fragment) in [(0, "20", "21"), (1, "30", "31")] {
        let group = document.fields()[field].group_object.unwrap();
        assert_eq!(
            Some(expected_group),
            document.objects()[group].id.as_deref()
        );
        assert_eq!(vec![expected_fragment], fragment_ids(&document, field));
    }
}

#[test]
fn different_compound_keys_in_one_group_are_ambiguous_and_never_guessed() {
    let document = drawing("<group><fragment/><t><s>[[@x]]</s></t><t><s>[[@y]]</s></t></group>");
    assert!(
        document
            .fields()
            .iter()
            .all(|field| field.fragment_objects.is_empty())
    );
    assert!(
        document
            .diagnostics()
            .iter()
            .any(|issue| issue.code == "ambiguous_field_group")
    );
}

#[test]
fn an_ungrouped_field_is_still_renderable_but_has_no_structure_binding() {
    let document = drawing("<fragment/><t><s>[[@x]]</s></t>");
    assert_eq!(None, document.fields()[0].group_object);
    assert!(document.fields()[0].fragment_objects.is_empty());
}

#[test]
fn inline_references_do_not_bind_a_structure_or_make_a_label_group_ambiguous() {
    let document = drawing(
        "<group><fragment id='2'/><t><s>[[@x]]</s></t><t><s>compare [[@y]]</s></t></group>",
    );
    assert_eq!(vec!["2"], fragment_ids(&document, 0));
    assert!(document.fields()[1].fragment_objects.is_empty());
    assert!(
        !document
            .diagnostics()
            .iter()
            .any(|issue| issue.code == "ambiguous_field_group")
    );
}

#[test]
fn fragments_inside_atom_abbreviations_are_not_separate_group_members() {
    let document = drawing(
        "<group><fragment id='2'><n NodeType='Nickname'><fragment id='3'><n/></fragment></n></fragment><t><s>[[@x]]</s></t></group>",
    );
    assert_eq!(vec!["2"], fragment_ids(&document, 0));
}

#[test]
fn compound_group_source_preserves_original_group_and_shared_styles() {
    let source = "<CDXML><fonttable><font id='1' name='Arial'/></fonttable><page id='1'><group id='10'><group id='20'><fragment id='21'/><t><s>[[@x]]</s></t></group><group id='30'><fragment id='31'/></group></group></page></CDXML>";
    let document = DrawingDocument::parse(source.into()).unwrap();

    let selected = document.compound_group_source("x").unwrap();

    assert_eq!(
        Some("20"),
        document.compound_group("x").unwrap().id.as_deref()
    );
    assert!(selected.contains("name='Arial'"));
    assert!(selected.contains("<group id='20'><fragment id='21'/><t><s>[[@x]]</s></t></group>"));
    assert!(!selected.contains("id='30'"));
    assert!(!selected.contains("id='31'"));
}

#[test]
fn compound_group_source_without_fields_keeps_the_structure_but_removes_labels() {
    let document = drawing(
        "<group id='10'><fragment id='11'/><t id='12'><s>[[@x]]</s></t></group><t><s>outside</s></t>",
    );

    let selected = document.compound_group_source_without_fields("x").unwrap();

    assert!(selected.contains("<fragment id='11'/>") || selected.contains("<fragment id=\"11\"/>"));
    assert!(!selected.contains("[[@x]]"));
    assert!(!selected.contains("outside"));
}

#[test]
fn compound_group_source_rejects_ungrouped_or_ambiguous_fields() {
    let ungrouped = drawing("<t><s>[[@x]]</s></t>");
    assert!(matches!(
        ungrouped.compound_group_source("x"),
        Err(chemex_cdxml::Error::MissingGroup(_))
    ));

    let ambiguous = drawing(
        "<group><fragment/><t><s>[[@x]]</s></t></group><group><fragment/><t><s>[[@x]]</s></t></group>",
    );
    assert!(matches!(
        ambiguous.compound_group_source("x"),
        Err(chemex_cdxml::Error::AmbiguousGroup(_))
    ));
}
