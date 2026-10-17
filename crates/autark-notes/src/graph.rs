//! The graph view: every note as a dot, sized by how many links it has, with a line for each
//! link. The open note and its neighbours stand out; clicking a dot opens that note.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use autark_common::theme;
use gtk::prelude::*;

use crate::editor::Callback;
use crate::vault::Vault;

/// Note paths, sorted, and links as pairs of indexes into them.
type Shape = (Vec<String>, Vec<(usize, usize)>);

struct Node {
    path: String,
    /// Position in the unit square, from the layout.
    x: f64,
    y: f64,
    size: f64,
    button: gtk::Button,
    dot: gtk::Box,
    label: gtk::Label,
}

pub struct Graph {
    pub widget: gtk::Overlay,
    area: gtk::DrawingArea,
    fixed: gtk::Fixed,
    caption: gtk::Label,
    nodes: RefCell<Vec<Node>>,
    edges: RefCell<Vec<(usize, usize)>>,
    /// The notes and links the layout was made for, so a new open note only recolours.
    shape: RefCell<Shape>,
    current: RefCell<Option<String>>,
    size: Cell<(f64, f64)>,
    on_open: Callback<String>,
}

const LABEL_WIDTH: f64 = 120.0;

impl Graph {
    pub fn new() -> Rc<Self> {
        let area = gtk::DrawingArea::builder().hexpand(true).vexpand(true).build();
        theme::redraw_on_change(&area);
        let fixed = gtk::Fixed::new();
        let caption = gtk::Label::new(None);
        caption.add_css_class("caption");
        caption.set_halign(gtk::Align::Start);
        caption.set_valign(gtk::Align::End);
        let widget = gtk::Overlay::new();
        widget.add_css_class("graph");
        widget.set_child(Some(&area));
        widget.add_overlay(&fixed);
        widget.add_overlay(&caption);
        let graph = Rc::new(Self {
            widget,
            area,
            fixed,
            caption,
            nodes: RefCell::default(),
            edges: RefCell::default(),
            shape: RefCell::default(),
            current: RefCell::default(),
            size: Cell::new((0.0, 0.0)),
            on_open: RefCell::default(),
        });
        let weak = Rc::downgrade(&graph);
        graph.area.connect_resize(move |_, width, height| {
            if let Some(graph) = weak.upgrade() {
                graph.size.set((width as f64, height as f64));
                graph.place();
            }
        });
        let weak = Rc::downgrade(&graph);
        graph.area.set_draw_func(move |_, cr, _, _| {
            let Some(graph) = weak.upgrade() else { return };
            let nodes = graph.nodes.borrow();
            let current = graph.current.borrow();
            cr.set_line_width(1.5);
            for &(a, b) in graph.edges.borrow().iter() {
                let touches = [a, b].iter().any(|&i| current.as_deref() == Some(nodes[i].path.as_str()));
                let palette = &theme::look().palette;
                if touches {
                    theme::set_source(cr, &palette.accent, 1.0);
                } else {
                    theme::set_source(cr, &palette.text, 0.18);
                }
                let ((ax, ay), (bx, by)) = (graph.point(&nodes[a]), graph.point(&nodes[b]));
                cr.move_to(ax, ay);
                cr.line_to(bx, by);
                let _ = cr.stroke();
            }
        });
        graph
    }

    pub fn connect_open(&self, f: impl Fn(String) + 'static) {
        *self.on_open.borrow_mut() = Some(Box::new(f));
    }

    /// Shows the vault's notes, laying them out again only when notes or links changed.
    pub fn show(self: &Rc<Self>, vault: &Vault, current: Option<&str>) {
        self.current.replace(current.map(str::to_owned));
        let mut paths: Vec<String> = vault.notes.iter().map(|note| note.path.clone()).collect();
        paths.sort();
        let mut edges = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            let Some(note) = vault.get(path) else { continue };
            for link in &note.links {
                let Some(target) = vault.resolve(link) else { continue };
                let Ok(j) = paths.binary_search(&target.path) else { continue };
                let edge = (i.min(j), i.max(j));
                if i != j && !edges.contains(&edge) {
                    edges.push(edge);
                }
            }
        }
        let shape = (paths, edges);
        if *self.shape.borrow() != shape {
            self.rebuild(vault, &shape.0, &shape.1);
            self.shape.replace(shape);
        }
        self.recolour();
    }

    fn rebuild(self: &Rc<Self>, vault: &Vault, paths: &[String], edges: &[(usize, usize)]) {
        for node in self.nodes.borrow().iter() {
            self.fixed.remove(&node.button);
        }
        let positions = layout(paths.len(), edges);
        let mut nodes = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            let degree = edges.iter().filter(|&&(a, b)| a == i || b == i).count();
            let size = (12.0 + 3.0 * degree as f64).min(36.0);
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("dot");
            dot.set_size_request(size as i32, size as i32);
            dot.set_halign(gtk::Align::Center);
            let title = vault.get(path).map(|note| note.title.clone()).unwrap_or_default();
            let label = gtk::Label::builder().label(&title).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(16).build();
            let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
            content.append(&dot);
            content.append(&label);
            let button = gtk::Button::builder().child(&content).tooltip_text(format!("Open {title}")).build();
            button.add_css_class("node");
            button.set_size_request(LABEL_WIDTH as i32, -1);
            let (weak, open) = (Rc::downgrade(self), path.clone());
            button.connect_clicked(move |_| {
                if let Some(graph) = weak.upgrade()
                    && let Some(on_open) = &*graph.on_open.borrow()
                {
                    on_open(open.clone());
                }
            });
            self.fixed.put(&button, 0.0, 0.0);
            let (x, y) = positions[i];
            nodes.push(Node { path: path.clone(), x, y, size, button, dot, label });
        }
        self.caption.set_label(&format!(
            "{} notes, {} links. Click a note to open it.",
            paths.len(),
            edges.len()
        ));
        self.nodes.replace(nodes);
        self.edges.replace(edges.to_vec());
        self.place();
    }

    fn recolour(&self) {
        let current = self.current.borrow();
        let nodes = self.nodes.borrow();
        let current_index = nodes.iter().position(|node| current.as_deref() == Some(node.path.as_str()));
        for (i, node) in nodes.iter().enumerate() {
            let near = current_index.is_some_and(|c| self.edges.borrow().iter().any(|&(a, b)| (a == c && b == i) || (b == c && a == i)));
            for (class, on) in [("current", Some(i) == current_index), ("near", near)] {
                for widget in [node.dot.upcast_ref::<gtk::Widget>(), node.label.upcast_ref()] {
                    if on {
                        widget.add_css_class(class);
                    } else {
                        widget.remove_css_class(class);
                    }
                }
            }
        }
        self.area.queue_draw();
    }

    /// A node's dot centre, in pixels.
    fn point(&self, node: &Node) -> (f64, f64) {
        let (width, height) = self.size.get();
        let (pad_x, pad_top, pad_bottom) = (LABEL_WIDTH / 2.0 + 10.0, 30.0, 60.0);
        (pad_x + node.x * (width - 2.0 * pad_x).max(0.0), pad_top + node.y * (height - pad_top - pad_bottom).max(0.0))
    }

    fn place(&self) {
        for node in self.nodes.borrow().iter() {
            let (x, y) = self.point(node);
            self.fixed.move_(&node.button, x - LABEL_WIDTH / 2.0, y - node.size / 2.0);
        }
        self.area.queue_draw();
    }
}

/// Fruchterman–Reingold: links pull, every pair pushes, and a little gravity keeps unlinked
/// notes in view. Deterministic, so the graph looks the same each time. Positions come back
/// spread over the unit square.
fn layout(count: usize, edges: &[(usize, usize)]) -> Vec<(f64, f64)> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![(0.5, 0.5)];
    }
    let k = (1.0 / count as f64).sqrt();
    let mut pos: Vec<(f64, f64)> = (0..count)
        .map(|i| {
            let angle = i as f64 * 2.399_963; // the golden angle spreads the start evenly
            let radius = 0.5 * ((i as f64 + 0.5) / count as f64).sqrt();
            (0.5 + radius * angle.cos(), 0.5 + radius * angle.sin())
        })
        .collect();
    let rounds = if count > 300 { 60 } else { 250 };
    for round in 0..rounds {
        let temperature = 0.1 * (1.0 - round as f64 / rounds as f64) + 0.002;
        let mut push = vec![(0.0, 0.0); count];
        for i in 0..count {
            for j in i + 1..count {
                let (dx, dy) = (pos[i].0 - pos[j].0, pos[i].1 - pos[j].1);
                let distance = (dx * dx + dy * dy).sqrt().max(0.001);
                let force = k * k / distance;
                let (fx, fy) = (dx / distance * force, dy / distance * force);
                push[i].0 += fx;
                push[i].1 += fy;
                push[j].0 -= fx;
                push[j].1 -= fy;
            }
        }
        for &(a, b) in edges {
            let (dx, dy) = (pos[a].0 - pos[b].0, pos[a].1 - pos[b].1);
            let distance = (dx * dx + dy * dy).sqrt().max(0.001);
            let force = distance * distance / k;
            let (fx, fy) = (dx / distance * force, dy / distance * force);
            push[a].0 -= fx;
            push[a].1 -= fy;
            push[b].0 += fx;
            push[b].1 += fy;
        }
        for (i, (fx, fy)) in push.into_iter().enumerate() {
            let (fx, fy) = (fx + (0.5 - pos[i].0) * 0.3, fy + (0.5 - pos[i].1) * 0.3);
            let length = (fx * fx + fy * fy).sqrt().max(0.000_001);
            let step = length.min(temperature);
            pos[i].0 += fx / length * step;
            pos[i].1 += fy / length * step;
        }
    }
    let (min_x, max_x) = pos.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
    let (min_y, max_y) = pos.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
    let scale = |v: f64, lo: f64, hi: f64| if hi - lo < 1e-9 { 0.5 } else { (v - lo) / (hi - lo) };
    pos.into_iter().map(|(x, y)| (scale(x, min_x, max_x), scale(y, min_y, max_y))).collect()
}

#[cfg(test)]
mod tests {
    use super::layout;

    #[test]
    fn linked_notes_sit_closer_than_unlinked_ones() {
        let pos = layout(4, &[(0, 1), (1, 2)]);
        let distance = |a: usize, b: usize| ((pos[a].0 - pos[b].0).powi(2) + (pos[a].1 - pos[b].1).powi(2)).sqrt();
        assert!(distance(0, 1) < distance(0, 3));
        assert!(pos.iter().all(|&(x, y)| (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)));
    }
}
