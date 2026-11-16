# Config schema derives

Rift's config doc comments are the source of Settings help. `ConfigSchema`
generates static field metadata and typed read/write functions at compile time;
there is no source parsing, reflection, or config-to-JSON conversion at runtime.
It is currently intended for named, non-generic Rift config structs.

```rust
#[derive(ConfigSchema)]
#[setting(group = "behavior")]
pub struct Example {
    /// Animate windows when their layout changes.
    #[setting(label = "Animate window changes", group = "behavior")]
    pub animate: bool,

    /// Duration of a window animation, in seconds.
    #[setting(label = "Duration", unit = "s", enabled_by = "animate")]
    pub duration: f64,

    /// Ratio of the screen width used by a column.
    #[setting(label = "Column width", scale = 100.0, unit = "%")]
    pub width: f64,

    /// Rules edited by a dedicated sheet.
    #[setting(custom)]
    pub rules: Vec<Rule>,

    #[setting(ignore)]
    pub internal_cache: Vec<String>,
}
```

- `bool`, numeric primitives, and `String` infer a native switch, number field,
  or text field. Names are humanized unless `label` overrides them.
- `#[setting(choices)]` uses the field enum's `ConfigEnum` metadata for a native
  popup. `ConfigEnum` derives labels and documentation from unit variants and
  allows `#[setting(label = "…")]` on variants. It works on protocol enums too,
  without depending on Rift or AppKit. Serde names and aliases are unaffected.
- `aliases` adds search terms alongside the field name and doc text.
- `group` selects a section; a struct-level group supplies the default for new
  fields. Source declaration order controls row order.
  `order` can override that order without rearranging the config struct.
- `enabled_by` refers to a boolean sibling field.
- `scale` changes display units only; typed values retain config units.
- `custom` retains metadata but delegates control creation to a specialized
  editor. Use it for inherited values, collections, paths, colors, and previews.
- `ignore` omits the field from the schema and generated search. Deprecated
  fields are also omitted. Unsupported fields require an explicit choice,
  rather than silently acquiring the wrong editor.

`FormBuilder::schema_section` takes read/write projections into `ConfigSource`
and creates controls with the existing ConfigActor validation, persistence,
and sheet-draft behavior. `schema_rows` fills a precomposed section; `schema_field` generates an individual row;
`schema_metadata` adds doc help and enabled-state dependencies to a custom row.
Help appears as native tooltips and Settings search descriptions, keeping forms
compact. Search registers schemas with a page/section destination once; adding
an ordinary field does not require adding it to a separate search-field list.

UI composition remains explicit for presentation and specialized flows. This
schema deliberately does not infer inheritance, collection operations, screen
geometry, or application inventory from a Rust field type.
