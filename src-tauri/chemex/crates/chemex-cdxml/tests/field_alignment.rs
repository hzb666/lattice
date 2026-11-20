use chemex_cdxml::{DrawingDocument, FieldValues};

fn render(attributes: &str, content: &str) -> (String, DrawingDocument) {
    let source = format!(
        "<CDXML CaptionJustification='Left'><page><t id='2' {attributes}>{content}</t><graphic id='3' BoundingBox='1 2 3 4'/></page></CDXML>"
    );
    let document = DrawingDocument::parse(source.clone()).unwrap();
    let mut values = FieldValues::new();
    values.insert("x".into(), "2a".into()).unwrap();
    let output = document.render_fields(&values).unwrap();
    assert_eq!(source, document.source());
    let rendered = DrawingDocument::parse(output.clone()).unwrap();
    assert_eq!(
        Some("1 2 3 4"),
        rendered.object_by_id("3").unwrap().attribute("BoundingBox")
    );
    (output, rendered)
}

#[test]
fn a_short_label_is_centered_on_the_original_box_instead_of_its_left_edge() {
    let (_, document) = render(
        "p='112.20 211.80' BoundingBox='112.20 202.85 222.25 214.35' LineHeight='auto'",
        "<s font='3' size='10' color='10' face='1'>[[@x]]</s>",
    );
    let text = document.object_by_id("2").unwrap();
    assert_eq!(Some("167.225 211.80"), text.attribute("p"));
    assert_eq!(Some("Center"), text.attribute("CaptionJustification"));
    assert_eq!(Some("Center"), text.attribute("Justification"));
    assert_eq!(None, text.attribute("BoundingBox"));
    assert_eq!("2a", document.texts()[0].text);
}

#[test]
fn already_centered_labels_keep_their_baseline_anchor_and_run_styles() {
    let (output, document) = render(
        "p='80 25' BoundingBox='30 15 130 28' CaptionJustification='Center' Justification='Left'",
        "<s font='3' color='4' face='1'>[[@</s><s size='12'>x]]</s>",
    );
    let text = document.object_by_id("2").unwrap();
    assert_eq!(Some("80 25"), text.attribute("p"));
    assert_eq!(Some("Center"), text.attribute("Justification"));
    assert!(
        output.contains("<s font='3' color='4' face='1'>2a</s><s size='12'></s>"),
        "{output}"
    );
}

#[test]
fn inherited_caption_alignment_takes_precedence_over_legacy_justification() {
    let source = "<CDXML CaptionJustification='Center'><page><t id='2' p='81 25' BoundingBox='30 15 130 28' Justification='Left'><s>[[@x]]</s></t></page></CDXML>";
    let document = DrawingDocument::parse(source.into()).unwrap();
    let mut values = FieldValues::new();
    values.insert("x".into(), "1".into()).unwrap();
    let rendered = DrawingDocument::parse(document.render_fields(&values).unwrap()).unwrap();
    assert_eq!(
        Some("81 25"),
        rendered.object_by_id("2").unwrap().attribute("p")
    );
}

#[test]
fn inline_fields_keep_authored_layout_attributes_and_surrounding_bytes() {
    let (output, _) = render(
        "p='10 20' BoundingBox='10 10 200 25' CaptionJustification='Right' LineStarts='0 8'",
        "<s>Compound [[@x]] (85%)</s>",
    );
    assert!(output.contains(
        "p='10 20' BoundingBox='10 10 200 25' CaptionJustification='Right' LineStarts='0 8'"
    ));
    assert!(output.contains("<s>Compound 2a (85%)</s>"));
}

#[test]
fn right_aligned_labels_use_the_box_center_and_drop_obsolete_line_offsets() {
    let (_, document) = render(
        "p='120 25' BoundingBox='20 15 120 28' CaptionJustification='Right' LineStarts='0 9'",
        "<s>[[@x]]</s>",
    );
    let text = document.object_by_id("2").unwrap();
    assert_eq!(Some("70 25"), text.attribute("p"));
    assert_eq!(None, text.attribute("LineStarts"));
}

#[test]
fn unsupported_or_missing_label_geometry_is_reported_and_left_untouched() {
    for attributes in [
        "p='10 20'",
        "p='10 20' BoundingBox='NaN 10 100 30'",
        "p='10 20' BoundingBox='0 0 100 30' RotationAngle='90'",
    ] {
        let original = DrawingDocument::parse(format!(
            "<CDXML><page><t {attributes}><s>[[@x]]</s></t></page></CDXML>"
        ))
        .unwrap();
        assert!(
            original
                .diagnostics()
                .iter()
                .any(|issue| issue.code == "field_alignment_unavailable")
        );
        let (output, document) = render(attributes, "<s>[[@x]]</s>");
        assert!(output.contains(attributes));
        assert_eq!("2a", document.texts()[0].text);
    }
}
