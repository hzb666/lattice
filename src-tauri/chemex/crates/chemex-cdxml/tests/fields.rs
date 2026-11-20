use chemex_cdxml::{DrawingDocument, Error, FieldValues};

fn values(pairs: &[(&str, &str)]) -> FieldValues {
    let mut values = FieldValues::new();
    for (key, value) in pairs {
        values.insert((*key).into(), (*value).into()).unwrap();
    }
    values
}

fn drawing(content: &str) -> String {
    format!(
        "<CDXML><page id='1'><t id='2' p='10 20' RotationAngle='90'>{content}</t></page></CDXML>"
    )
}

#[test]
fn replaces_repeated_fields_across_pages_and_styled_runs() {
    let source = include_str!("fixtures/fields.cdxml");
    let document = DrawingDocument::parse(source.into()).unwrap();
    let mapping = values(&[("compound-001", "1"), ("compound-002", "2a")]);
    let expected = source
        .replace("[[@compound-001]]", "1")
        .replace("[[@compound-", "2a")
        .replace("002]]", "")
        .replace("BoundingBox=\"24 48 180 60\"", "")
        .replace("BoundingBox=\"-31 48 79 60\"", "");
    assert_eq!(3, document.fields().len());
    assert_eq!(expected, document.render_fields(&mapping).unwrap());
    assert_eq!(source, document.source());
}

#[test]
fn keeps_all_bytes_outside_a_field_including_unusual_entity_spelling() {
    let source = drawing("<s font='3' size='10'>中文 &#x26; &#38; [[@x]] &quot;尾部&quot;</s>");
    let document = DrawingDocument::parse(source.clone()).unwrap();
    let expected = source.replace("[[@x]]", "A&amp;B&lt;C&gt;");
    assert_eq!(
        expected,
        document.render_fields(&values(&[("x", "A&B<C>")])).unwrap()
    );
}

#[test]
fn fields_may_use_xml_character_references_for_the_brackets() {
    let source = drawing("<s>&#91;&#x5b;@x&#93;&#x5d;</s>");
    let document = DrawingDocument::parse(source.clone()).unwrap();
    assert_eq!(1, document.fields().len());
    assert_eq!(
        drawing("<s>2b</s>"),
        document.render_fields(&values(&[("x", "2b")])).unwrap()
    );
}

#[test]
fn retains_the_first_style_when_a_field_spans_different_styles() {
    let source = drawing("<s face='1'>before [[@x</s><s face='2'>]] after</s>");
    let expected = drawing("<s face='1'>before 3a</s><s face='2'> after</s>");
    let document = DrawingDocument::parse(source).unwrap();
    assert_eq!(
        expected,
        document.render_fields(&values(&[("x", "3a")])).unwrap()
    );
}

#[test]
fn supports_cdata_and_safely_splits_a_cdata_terminator_in_a_label() {
    let source = drawing("<s><![CDATA[prefix [[@x]] suffix]]></s>");
    let document = DrawingDocument::parse(source).unwrap();
    let output = document.render_fields(&values(&[("x", "A]]>B")])).unwrap();
    let rendered = DrawingDocument::parse(output).unwrap();
    assert_eq!("prefix A]]>B suffix", rendered.texts()[0].text);
    assert_eq!(0, rendered.fields().len());
}

#[test]
fn adjacent_text_and_cdata_do_not_lose_their_source_ranges() {
    let source = drawing("<s>before [[@<![CDATA[x]]]]> after</s>");
    let document = DrawingDocument::parse(source).unwrap();
    let output = document.render_fields(&values(&[("x", "1")])).unwrap();
    let rendered = DrawingDocument::parse(output).unwrap();
    assert_eq!("before 1 after", rendered.texts()[0].text);
}

#[test]
fn preserves_comments_processing_instructions_crlf_and_bom() {
    let source = format!(
        "\u{feff}{}",
        drawing("<s>前\r\n[[@<!--keep--><?keep data?>x]]\r\n后</s>")
    );
    let document = DrawingDocument::parse(source.clone()).unwrap();
    let expected = source.replace("[[@", "7").replace("x]]", "");
    assert_eq!(
        expected,
        document.render_fields(&values(&[("x", "7")])).unwrap(),
        "{source}"
    );
}

#[test]
fn does_not_replace_fields_in_atom_labels_attributes_or_comments() {
    let source = "<CDXML><!-- [[@x]] --><page><fragment><n id='1'><t id='2'><s>[[@x]]</s></t></n></fragment><graphic custom='[[@x]]'/></page></CDXML>";
    let document = DrawingDocument::parse(source.into()).unwrap();
    assert!(document.fields().is_empty());
    assert!(
        document
            .diagnostics()
            .iter()
            .any(|issue| issue.code == "field_in_atom_label")
    );
    assert_eq!(
        source,
        document.render_fields(&values(&[("x", "1")])).unwrap()
    );
}

#[test]
fn multiple_fields_in_one_text_are_independent() {
    let source = drawing("<s>[[@x]] + [[@y]] = [[@x]]</s>");
    let expected = drawing("<s>1 + 2a = 1</s>");
    let document = DrawingDocument::parse(source).unwrap();
    assert_eq!(
        expected,
        document
            .render_fields(&values(&[("x", "1"), ("y", "2a")]))
            .unwrap()
    );
}

#[test]
fn a_new_mapping_renders_from_the_original_placeholders() {
    let source = drawing("<s>[[@x]]</s>");
    let document = DrawingDocument::parse(source.clone()).unwrap();
    assert_eq!(
        drawing("<s>1</s>"),
        document.render_fields(&values(&[("x", "1")])).unwrap()
    );
    assert_eq!(
        drawing("<s>2b</s>"),
        document.render_fields(&values(&[("x", "2b")])).unwrap()
    );
    assert_eq!(source, document.source());
}

#[test]
fn prefixes_and_suffixes_survive_replacement_in_their_authored_style_runs() {
    let source = drawing("<s face='1'>[[@x_ Scheme ^</s><s face='2'> product ]]</s>");
    let document = DrawingDocument::parse(source).unwrap();
    let output = document.render_fields(&values(&[("x", "IV")])).unwrap();
    let rendered = DrawingDocument::parse(output.clone()).unwrap();
    assert_eq!(" Scheme IV product ", rendered.texts()[0].text);
    assert!(
        output.contains("<s face='1'> Scheme IV</s><s face='2'> product </s>"),
        "{output}"
    );
}

#[test]
fn missing_or_malformed_fields_fail_without_mutating_the_source() {
    for content in ["[[@x]]", "[[@bad key]]", "[[@]]", "[[@x"] {
        let source = drawing(&format!("<s>{content}</s>"));
        let document = DrawingDocument::parse(source.clone()).unwrap();
        assert!(
            document.render_fields(&FieldValues::new()).is_err(),
            "{content}"
        );
        assert_eq!(source, document.source());
    }
}

#[test]
fn a_custom_entity_is_inspectable_but_not_guessed_during_replacement() {
    let source = "<!DOCTYPE CDXML [<!ENTITY prefix 'label'>]><CDXML><page><t><s>&prefix; [[@x]]</s></t></page></CDXML>";
    let document = DrawingDocument::parse(source.into()).unwrap();
    assert_eq!("label [[@x]]", document.texts()[0].text);
    assert!(matches!(
        document.render_fields(&values(&[("x", "1")])),
        Err(Error::TextMapping { .. })
    ));
}

#[test]
fn rejects_invalid_keys_labels_and_duplicate_mapping_entries() {
    let mut mapping = FieldValues::new();
    for key in ["", "bad key", "{x}", "\\x", "x%"] {
        assert!(mapping.insert(key.into(), "1".into()).is_err());
    }
    for label in [
        "", " ", "2\n3", "2\r3", "2\t3", "\0", "\u{fffe}", "\u{2028}",
    ] {
        assert!(mapping.insert("x".into(), label.into()).is_err());
    }
    mapping.insert("x".into(), "1".into()).unwrap();
    assert!(matches!(
        mapping.insert("x".into(), "2".into()),
        Err(Error::DuplicateKey(_))
    ));
}

#[test]
fn leaves_text_in_unknown_objects_and_metadata_untouched() {
    let source = "<CDXML><page><vendor><t><s>[[@x]]</s></t></vendor><fragment><objecttag Name='custom'><t><s>[[@x]]</s></t></objecttag></fragment></page></CDXML>";
    let document = DrawingDocument::parse(source.into()).unwrap();
    assert!(document.fields().is_empty());
    assert_eq!(
        source,
        document.render_fields(&values(&[("x", "1")])).unwrap()
    );
}

#[test]
fn does_not_guess_how_to_patch_unknown_nested_style_content() {
    let source = drawing("<s>[[@<vendor>x</vendor>]]</s>");
    let document = DrawingDocument::parse(source.clone()).unwrap();
    assert!(matches!(
        document.render_fields(&values(&[("x", "1")])),
        Err(Error::TextMapping { .. })
    ));
    assert_eq!(source, document.source());
}
