use chemex_cdxml::{DrawingDocument, FieldValues, ObjectKind};

const FIXTURE: &str = include_str!("fixtures/fields.cdxml");

#[test]
fn indexes_pages_groups_graphics_and_unknown_objects_without_rewriting() {
    let document = DrawingDocument::parse(FIXTURE.into()).unwrap();
    assert_eq!(FIXTURE, document.source());
    assert_eq!(
        2,
        document
            .objects()
            .iter()
            .filter(|object| object.kind == ObjectKind::Page)
            .count()
    );
    let group = document
        .objects()
        .iter()
        .position(|object| object.id.as_deref() == Some("2"))
        .unwrap();
    let graphic = document.object_by_id("10").unwrap();
    assert_eq!(Some(group), graphic.parent);
    assert_eq!(Some("FullHead"), graphic.attribute("ArrowType"));
    assert!(
        document
            .objects()
            .iter()
            .any(|object| object.element == "vendorobject")
    );
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn extracts_authored_chemistry_and_preserves_stereo_display() {
    let document = DrawingDocument::parse(FIXTURE.into()).unwrap();
    let fragment = &document.fragments()[0];
    assert_eq!(2, fragment.atoms.len());
    assert_eq!(Some(13), fragment.atoms[0].isotope);
    assert_eq!(Some([20.0, 30.0]), fragment.atoms[0].position);
    assert_eq!(Some(-1), fragment.atoms[1].charge);
    assert_eq!("O", fragment.atoms[1].label);
    assert_eq!("1", fragment.bonds[0].order);
    assert_eq!(Some("WedgeBegin"), fragment.bonds[0].display.as_deref());
}

#[test]
fn reads_object_tags_without_treating_them_as_verified_persistent_identity() {
    let document = DrawingDocument::parse(FIXTURE.into()).unwrap();
    let tag = &document.object_tags()[0];
    assert_eq!(Some("com.chemex.compound-id"), tag.name.as_deref());
    assert_eq!(Some("compound-001"), tag.value.as_deref());
    let owner = &document.objects()[tag.owner.unwrap()];
    assert_eq!(ObjectKind::Fragment, owner.kind);
    assert_eq!(Some("3"), owner.id.as_deref());
}

#[test]
fn unsupported_chemistry_does_not_block_dynamic_text() {
    let source = "<CDXML><page id='1'><fragment id='2'><n id='3' NodeType='GenericNickname' p='NaN 4'/><b id='4' B='3' E='99' Order='any'/></fragment><t id='5'><s>[[@x]]</s></t></page></CDXML>";
    let document = DrawingDocument::parse(source.into()).unwrap();
    let mut values = FieldValues::new();
    values.insert("x".into(), "2a".into()).unwrap();
    assert_eq!(None, document.fragments()[0].atoms[0].atomic_number);
    assert_eq!("any", document.fragments()[0].bonds[0].order);
    assert!(
        document
            .diagnostics()
            .iter()
            .any(|issue| issue.code == "unsupported_node_type")
    );
    assert!(
        document
            .diagnostics()
            .iter()
            .any(|issue| issue.code == "invalid_bond_reference")
    );
    assert_eq!(
        source.replace("[[@x]]", "2a"),
        document.render_fields(&values).unwrap()
    );
}

#[test]
fn duplicate_object_ids_are_ambiguous_but_font_ids_are_a_separate_namespace() {
    let source = "<CDXML><fonttable><font id='3'/></fonttable><page id='1'><fragment id='2'><n id='3'/><n id='3'/><b id='4' B='3' E='3'/></fragment></page></CDXML>";
    let document = DrawingDocument::parse(source.into()).unwrap();
    assert!(document.object_by_id("3").is_none());
    assert_eq!(
        1,
        document
            .diagnostics()
            .iter()
            .filter(|issue| issue.code == "duplicate_object_id")
            .count()
    );
    assert_eq!(source, document.render_fields(&FieldValues::new()).unwrap());
}

#[test]
fn nested_fragments_remain_separate_authored_containers() {
    let source = "<CDXML><page><group><fragment id='1'><n id='2' NodeType='Nickname'><fragment id='3'><n id='4'/></fragment></n></fragment></group></page></CDXML>";
    let document = DrawingDocument::parse(source.into()).unwrap();
    assert_eq!(2, document.fragments().len());
    assert_eq!(1, document.fragments()[0].atoms.len());
    assert_eq!(1, document.fragments()[1].atoms.len());
}

#[test]
fn invalid_atom_attributes_are_reported_instead_of_defaulted() {
    let source = "<CDXML><page><fragment><n id='1' Element='oops' Charge='x' Isotope='bad' p='x 4'/></fragment></page></CDXML>";
    let document = DrawingDocument::parse(source.into()).unwrap();
    let atom = &document.fragments()[0].atoms[0];
    assert_eq!(None, atom.atomic_number);
    assert_eq!(None, atom.charge);
    assert_eq!(None, atom.isotope);
    assert_eq!(None, atom.position);
    assert_eq!(
        Some("oops"),
        document.object_by_id("1").unwrap().attribute("Element")
    );
}

#[test]
fn rejects_wrong_roots_malformed_xml_and_misdeclared_encoding() {
    for source in [
        "<svg/>",
        "<CDXML><page></CDXML>",
        "<CDXML xmlns='urn:other'/>",
        "<?xml version='1.0' encoding='UTF-16'?><CDXML/>",
    ] {
        assert!(DrawingDocument::parse(source.into()).is_err(), "{source}");
    }
}

#[test]
fn does_not_resolve_external_entities_from_a_file_or_url() {
    for address in [
        "file:///C:/Windows/win.ini",
        "https://invalid.example/entity",
    ] {
        let source = format!(
            "<!DOCTYPE CDXML [<!ENTITY external SYSTEM '{address}'>]><CDXML><page><t><s>&external;</s></t></page></CDXML>"
        );
        assert!(DrawingDocument::parse(source).is_err());
    }
}

#[test]
fn native_chemdraw_fixture_preserves_the_original_source() {
    for source in [
        include_str!("fixtures/rdkit/mol1.cdxml"),
        include_str!("fixtures/rdkit/Cp-.cdxml"),
    ] {
        let document = DrawingDocument::parse(source.into()).unwrap();
        assert_eq!(
            Some("ChemDraw 6.0.1"),
            document.objects()[0].attribute("CreationProgram")
        );
        assert_eq!(5, document.fragments()[0].atoms.len());
        assert_eq!(5, document.fragments()[0].bonds.len());
        assert_eq!(Some(-1), document.fragments()[0].atoms[4].charge);
        assert!(
            document.diagnostics().is_empty(),
            "{:?}",
            document.diagnostics()
        );
        assert_eq!(source, document.render_fields(&FieldValues::new()).unwrap());
    }
}

#[test]
fn rejects_oversized_sources_before_building_an_xml_tree() {
    let source = " ".repeat(chemex_cdxml::MAX_SOURCE_BYTES + 1);
    assert!(matches!(
        DrawingDocument::parse(source),
        Err(chemex_cdxml::Error::SourceTooLarge { .. })
    ));
}
