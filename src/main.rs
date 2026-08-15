mod scan;

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph};
use scan::{FileEntry, human_size, scan_top_files};
use std::path::PathBuf;
use std::time::Duration;

/// List the largest files on disk and send the ones you pick to the trash.
#[derive(Parser, Debug)]
#[clap(
    name = env!("CARGO_PKG_NAME"),
    version = env!("CARGO_PKG_VERSION"),
    about = env!("CARGO_PKG_DESCRIPTION"),
)]
struct Args {
    /// Directory to scan. Stays on its filesystem (won't cross into other
    /// mounts, network shares, /proc, /sys, ...).
    #[arg(default_value = "/")]
    path: PathBuf,

    /// How many of the largest files to show
    #[arg(short, long, default_value_t = 100)]
    top: usize,
}

struct App {
    files: Vec<FileEntry>,
    total_space: u64,
    list_state: ListState,
    /// Index into `files` awaiting a y/n confirmation, if any.
    confirm_delete: Option<usize>,
    status: Option<String>,
}

impl App {
    fn new(files: Vec<FileEntry>, total_space: u64) -> App {
        let mut list_state = ListState::default();
        if !files.is_empty() {
            list_state.select(Some(0));
        }
        App {
            files,
            total_space,
            list_state,
            confirm_delete: None,
            status: None,
        }
    }

    fn move_selection(&mut self, forward: bool) {
        if self.files.is_empty() {
            return;
        }
        let len = self.files.len();
        let i = self.list_state.selected().unwrap_or(0);
        let next = if forward {
            (i + 1) % len
        } else {
            (i + len - 1) % len
        };
        self.list_state.select(Some(next));
    }

    fn request_delete(&mut self) {
        if let Some(i) = self.list_state.selected()
            && i < self.files.len()
        {
            self.confirm_delete = Some(i);
        }
    }

    fn cancel_delete(&mut self) {
        self.confirm_delete = None;
    }

    fn confirm_delete_now(&mut self) {
        let Some(i) = self.confirm_delete.take() else {
            return;
        };
        let Some(file) = self.files.get(i) else {
            return;
        };

        match trash::delete(&file.path) {
            Ok(()) => {
                self.status = Some(format!("Moved to trash: {}", file.path.display()));
                self.files.remove(i);
                if self.files.is_empty() {
                    self.list_state.select(None);
                } else {
                    self.list_state.select(Some(i.min(self.files.len() - 1)));
                }
            }
            Err(e) => {
                self.status = Some(format!("Failed to trash {}: {}", file.path.display(), e));
            }
        }
    }
}

fn main() {
    let args = Args::parse();

    if !args.path.exists() {
        eprintln!("Error: {} does not exist", args.path.display());
        std::process::exit(1);
    }

    let total_space = fs4::total_space(&args.path).unwrap_or(0);

    eprintln!("Scanning {} ...", args.path.display());
    let result = scan_top_files(&args.path, args.top);
    eprintln!(
        "Scanned {} files ({} unreadable, skipped). Showing the {} largest.",
        result.files_scanned,
        result.scan_errors,
        result.top_files.len()
    );

    if result.top_files.is_empty() {
        eprintln!("No files found under {}.", args.path.display());
        return;
    }

    let mut app = App::new(result.top_files, total_space);

    let mut terminal = ratatui::init();
    let res = run(&mut terminal, &mut app);
    ratatui::restore();

    if let Err(e) = res {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    loop {
        terminal.draw(|frame| draw(frame, app))?;

        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if app.confirm_delete.is_some() {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_delete_now(),
                    _ => app.cancel_delete(),
                }
                continue;
            }

            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Down => app.move_selection(true),
                KeyCode::Up => app.move_selection(false),
                KeyCode::Enter | KeyCode::Char('d') | KeyCode::Delete => app.request_delete(),
                _ => {}
            }
        }
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    let header = format!(
        "diskhog — {} files, disk {}   ↑/↓: select, Enter/d: delete (trash), q: quit",
        app.files.len(),
        human_size(app.total_space)
    );
    frame.render_widget(Line::from(header), layout[0]);

    let items: Vec<ListItem> = app
        .files
        .iter()
        .map(|f| {
            let pct = if app.total_space > 0 {
                f.size as f64 / app.total_space as f64 * 100.0
            } else {
                0.0
            };
            ListItem::new(format!(
                "{:>10}  {:>6.2}%  {}",
                human_size(f.size),
                pct,
                f.path.display()
            ))
        })
        .collect();

    let list = List::new(items)
        .block(Block::bordered().title("Largest files"))
        .highlight_style(Style::default().reversed())
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, layout[1], &mut app.list_state);

    let status = app.status.clone().unwrap_or_default();
    frame.render_widget(Line::from(status), layout[2]);

    if let Some(i) = app.confirm_delete
        && let Some(file) = app.files.get(i)
    {
        draw_confirm_popup(frame, file);
    }
}

fn draw_confirm_popup(frame: &mut Frame, file: &FileEntry) {
    let area = centered_rect(70, 9, frame.area());
    frame.render_widget(Clear, area);

    let text = format!(
        "Move to trash?\n\n{}\n({})\n\ny: yes    any other key: cancel",
        file.path.display(),
        human_size(file.size)
    );
    let popup = Paragraph::new(text)
        .wrap(ratatui::widgets::Wrap { trim: false })
        .block(Block::bordered().title("Confirm delete"));
    frame.render_widget(popup, area);
}

/// A Rect `pct_x`% as wide as `area` and `rows` tall, centered within it.
fn centered_rect(pct_x: u16, rows: u16, area: Rect) -> Rect {
    let rows = rows.min(area.height);
    let vertical = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(rows),
        Constraint::Fill(1),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn entry(name: &str, size: u64) -> FileEntry {
        FileEntry {
            path: PathBuf::from(name),
            size,
        }
    }

    fn sample_app() -> App {
        App::new(vec![entry("a", 30), entry("b", 20), entry("c", 10)], 1000)
    }

    #[test]
    fn selection_wraps_forward_and_backward() {
        let mut app = sample_app();
        assert_eq!(app.list_state.selected(), Some(0));
        app.move_selection(true);
        app.move_selection(true);
        assert_eq!(app.list_state.selected(), Some(2));
        app.move_selection(true);
        assert_eq!(
            app.list_state.selected(),
            Some(0),
            "should wrap past the last file"
        );
        app.move_selection(false);
        assert_eq!(
            app.list_state.selected(),
            Some(2),
            "should wrap backward past the first file"
        );
    }

    #[test]
    fn selection_on_empty_list_does_not_panic() {
        let mut app = App::new(Vec::new(), 1000);
        assert_eq!(app.list_state.selected(), None);
        app.move_selection(true);
        app.move_selection(false);
        assert_eq!(app.list_state.selected(), None);
    }

    #[test]
    fn request_delete_arms_confirmation_for_the_selected_file() {
        let mut app = sample_app();
        app.move_selection(true); // select index 1 ("b")
        app.request_delete();
        assert_eq!(app.confirm_delete, Some(1));
    }

    #[test]
    fn cancel_delete_clears_confirmation_without_touching_files() {
        let mut app = sample_app();
        app.request_delete();
        app.cancel_delete();
        assert_eq!(app.confirm_delete, None);
        assert_eq!(app.files.len(), 3);
    }

    #[test]
    fn confirm_delete_moves_the_real_file_to_trash_and_removes_it_from_the_list() {
        let dir = std::env::temp_dir().join(format!("diskhog-app-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("delete_me.bin");
        fs::write(&target, vec![0u8; 123]).unwrap();

        let mut app = App::new(vec![entry(target.to_str().unwrap(), 123)], 1000);
        app.request_delete();
        assert_eq!(app.confirm_delete, Some(0));

        app.confirm_delete_now();

        assert_eq!(app.confirm_delete, None);
        assert!(
            app.files.is_empty(),
            "the deleted file should be removed from the list"
        );
        assert!(
            !target.exists(),
            "the file should be gone from its original location"
        );
        assert!(
            app.status
                .as_deref()
                .unwrap_or("")
                .contains("Moved to trash"),
            "status should confirm the trash move: {:?}",
            app.status
        );

        let _ = fs::remove_dir_all(&dir);
        let _ = trash::delete(&target); // no-op if already gone; best-effort cleanup of any trash entry
    }

    #[test]
    fn confirm_delete_keeps_selection_in_bounds_after_removing_the_last_file() {
        let dir =
            std::env::temp_dir().join(format!("diskhog-app-test-bounds-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let first = dir.join("keep.bin");
        let last = dir.join("delete_me.bin");
        fs::write(&first, vec![0u8; 5]).unwrap();
        fs::write(&last, vec![0u8; 5]).unwrap();

        let mut app = App::new(
            vec![
                entry(first.to_str().unwrap(), 5),
                entry(last.to_str().unwrap(), 5),
            ],
            1000,
        );
        app.list_state.select(Some(1)); // the last file
        app.request_delete();
        app.confirm_delete_now();

        assert_eq!(app.files.len(), 1);
        assert_eq!(
            app.list_state.selected(),
            Some(0),
            "selection should clamp to the new last index, not point past the end"
        );

        let _ = fs::remove_dir_all(&dir);
        let _ = trash::delete(&last);
    }
}
