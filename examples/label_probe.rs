// Probe: how does a GtkLabel behave with wrap + ellipsize + max_width_chars,
// with and without `lines`? Prints measured heights so we can confirm which
// combination actually wraps to multiple lines.
use gtk::prelude::*;

fn make(text: &str, wrap: bool, ell: Option<gtk::pango::EllipsizeMode>, lines: Option<i32>) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_text(text);
    l.set_width_chars(20);
    l.set_max_width_chars(20);
    if wrap {
        l.set_wrap(true);
    }
    if let Some(e) = ell {
        l.set_ellipsize(e);
    }
    if let Some(n) = lines {
        l.set_lines(n);
    }
    l.show();
    l
}

fn main() {
    gtk::init().unwrap();
    let text = "这是一条很长很长的通知正文，模拟滴答清单任务完成通知，包含大量中文字符与一些 english words mixed in，用来观察换行和省略行为差异是否明显可测量。";
    let long = format!("{text}{}", text.repeat(3));
    let cases: Vec<(&str, bool, Option<gtk::pango::EllipsizeMode>, Option<i32>, &str)> = vec![
        ("short wrap+middle+lines5", true, Some(gtk::pango::EllipsizeMode::Middle), Some(5), text),
        ("short wrap+end+lines5", true, Some(gtk::pango::EllipsizeMode::End), Some(5), text),
        ("LONG wrap+middle+lines5", true, Some(gtk::pango::EllipsizeMode::Middle), Some(5), &long),
        ("LONG wrap+end+lines5", true, Some(gtk::pango::EllipsizeMode::End), Some(5), &long),
    ];
    let width = 300;
    for (name, wrap, ell, lines, t) in cases {
        let l = make(t, wrap, ell, lines);
        let (_, min_h) = l.preferred_height();
        let (_, h_at_w) = l.preferred_height_for_width(width);
        let layout = l.layout().unwrap();
        layout.set_width(width * gtk::pango::SCALE);
        let nlines = layout.line_count();
        println!(
            "{name}: natural_h={min_h} h@{width}={h_at_w} lines@{width}={nlines}"
        );
    }
}
