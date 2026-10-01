//! The comrak options every render uses.

use comrak::Options;

pub(crate) fn comrak_options() -> Options<'static> {
    let mut options = Options::default();

    let ext = &mut options.extension;
    ext.strikethrough = true;
    ext.table = true;
    ext.autolink = true;
    ext.tasklist = true;
    ext.footnotes = true;
    ext.front_matter_delimiter = Some("---".to_owned());
    ext.wikilinks_title_after_pipe = true;
    ext.alerts = true;
    // `header_id_prefix` stays off: heading ids come from our own slugger.

    // `data-sourcepos` anchors scrolling, reading position, search jumps and live reload.
    options.render.sourcepos = true;
    // Raw HTML passes through here; `sanitize::clean` makes the final HTML safe.
    options.render.r#unsafe = true;

    options
}
