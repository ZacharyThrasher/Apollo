//! Settings > Data: export the library, and create a diagnostics report.
//!
//! "Export library" is a select row (CSV or JSON, JSON by default) with an Export… button that
//! opens the Windows save dialog; "Diagnostics report" is a label with a Create… button that
//! does the same for a .txt file. While a job runs its button reads "Exporting…" / "Creating…"
//! and is disabled. Nothing says it worked: the file is the result. A failure shows beside the
//! button, in text.secondary, until the next try.

use std::path::{Path, PathBuf};

use apollo_account::service::IdentityState;
use apollo_services_common::unix_now;
use apollo_theme::tokens::{spacing, typography};
use gpui::{
    div, prelude::*, px, Context, Entity, FocusHandle, IntoElement, ParentElement, Render,
    SharedString, Styled, Window,
};

use crate::account::AccountModel;
use crate::diagnostics::{self, time::date_utc};
use crate::export::{self, collect::Options, ExportFormat};
use crate::player::PlayerModel;
use crate::settings::{SettingsModel, SqliteSettings, EXPORT_FORMAT};
use crate::style::{self, color};
use crate::ui::account_page::TAB_INDEX;
use crate::ui::choice_menu::{self, Choice};
use crate::ui::settings_row::{anchor_key, SettingsRow, SettingsSection};
use crate::ui::text_button::TextButton;

/// Row ids. The export row's format popover hangs from `anchor_key(ROW_EXPORT)`.
pub const ROW_EXPORT: &str = "setting-export-library";
pub const ROW_REPORT: &str = "setting-diagnostics-report";
pub const BUTTON_EXPORT: &str = "setting-export-button";
pub const BUTTON_REPORT: &str = "setting-report-button";

const FORMAT_MENU_WIDTH: f32 = 140.0;
/// The most an inline error may take before it is cut with an ellipsis.
const ERROR_MAX_WIDTH: f32 = 240.0;

/// Where a job stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Job {
    #[default]
    Idle,
    Running,
    /// What went wrong, in a few words.
    Failed(SharedString),
}

impl Job {
    pub fn is_running(&self) -> bool {
        *self == Job::Running
    }
}

pub struct DataPage {
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    account: Entity<AccountModel>,
    export: Job,
    report: Job,
    focus_format: FocusHandle,
    focus_export: FocusHandle,
    focus_report: FocusHandle,
}

impl DataPage {
    pub fn new(
        player: Entity<PlayerModel>,
        settings: Entity<SettingsModel>,
        account: Entity<AccountModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        let stop = |cx: &mut Context<Self>| cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true);
        Self {
            focus_format: stop(cx),
            focus_export: stop(cx),
            focus_report: stop(cx),
            player,
            settings,
            account,
            export: Job::Idle,
            report: Job::Idle,
        }
    }

    pub fn export_job(&self) -> &Job {
        &self.export
    }

    pub fn report_job(&self) -> &Job {
        &self.report
    }

    fn format(&self, cx: &gpui::App) -> ExportFormat {
        ExportFormat::from_stored(&self.settings.read(cx).get_text(&EXPORT_FORMAT))
    }

    fn open_formats(&self, window: &mut Window, cx: &mut gpui::App) {
        let current = self.format(cx);
        let choices = ExportFormat::ALL
            .iter()
            .map(|f| Choice::new(f.label(), *f == current))
            .collect();
        let settings = self.settings.clone();
        choice_menu::open(
            &anchor_key(ROW_EXPORT),
            FORMAT_MENU_WIDTH,
            choices,
            move |index, _, cx| {
                if let Some(format) = ExportFormat::ALL.get(index) {
                    settings.update(cx, |s, cx| s.set_text(&EXPORT_FORMAT, format.stored(), cx));
                }
            },
            window,
            cx,
        );
    }

    /// Export…: asks where, then reads the library and writes the file.
    pub fn start_export(&mut self, cx: &mut Context<Self>) {
        if self.export.is_running() {
            return;
        }
        let Some(source) = self.player.read(cx).library_source() else {
            self.export = Job::Failed("Not ready yet".into());
            cx.notify();
            return;
        };
        let format = self.format(cx);
        let now = unix_now();
        let picked = cx.prompt_for_new_path(
            &default_directory(),
            Some(&export::suggested_name(format, now)),
        );
        cx.spawn(async move |this, cx| {
            // Cancelling the dialog is not a failure: nothing changes.
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |page, cx| {
                page.export = Job::Running;
                cx.notify();
            });
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    export::export_to_file(&*source, &path, format, now, Options::default())
                })
                .await;
            let _ = this.update(cx, |page, cx| {
                page.export = match outcome {
                    Ok(None) => Job::Idle,
                    Ok(Some(text)) => Job::Failed(text.into()),
                    Err(text) => Job::Failed(text.into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Create…: asks where, then gathers the facts, scrubs them and writes the text file.
    pub fn start_report(&mut self, cx: &mut Context<Self>) {
        if self.report.is_running() {
            return;
        }
        let snapshot = self.snapshot(cx);
        let now = unix_now();
        let name = format!("apollo-diagnostics-{}.txt", date_utc(now));
        let picked = cx.prompt_for_new_path(&default_directory(), Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |page, cx| {
                page.report = Job::Running;
                cx.notify();
            });
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let text = diagnostics::build(&diagnostics::gather(snapshot));
                    write_text(&path, &text)
                })
                .await;
            let _ = this.update(cx, |page, cx| {
                page.report = match outcome {
                    Ok(()) => Job::Idle,
                    Err(text) => Job::Failed(text.into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// What the report needs from the UI side: the account's state (never who), the names to
    /// scrub, and where the settings and the database are.
    fn snapshot(&self, cx: &gpui::App) -> diagnostics::Snapshot {
        let view = self.account.read(cx).service().view();
        let mut known_secrets = Vec::new();
        let mut note = |identity: &apollo_account::identity::Identity| {
            known_secrets.push(identity.name.clone());
            known_secrets.extend(identity.handle.clone());
            known_secrets.extend(identity.page_id.clone());
        };
        let identity = match &view.identity {
            IdentityState::None => "none",
            IdentityState::Selected(identity) => {
                note(identity);
                "chosen"
            }
            IdentityState::Ambiguous(all) | IdentityState::Changed(all) => {
                all.iter().for_each(&mut note);
                "needs choosing"
            }
        };
        diagnostics::Snapshot {
            account: format!(
                "session {:?}, identity {identity}, {}",
                view.session,
                view.premium_label()
            ),
            known_secrets,
            settings: self.settings.read(cx).backend(),
            db_path: SqliteSettings::default_location().map(|s| s.path().to_path_buf()),
        }
    }
}

/// Where the save dialog opens: Documents when there is one.
fn default_directory() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|p| PathBuf::from(p).join("Documents"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

fn write_text(path: &Path, text: &str) -> Result<(), &'static str> {
    let path = if path.extension().is_some() {
        path.to_owned()
    } else {
        path.with_extension("txt")
    };
    std::fs::write(path, text).map_err(|_| "Couldn't write the file")
}

/// The inline error, when there is one, then the button.
fn control(
    error: &Job,
    button: TextButton,
    button_id: &'static str,
    cx: &gpui::App,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing::S3))
        .children(match error {
            Job::Failed(text) => Some(
                style::text_style(div(), typography::CAPTION, cx)
                    .debug_selector(|| format!("{button_id}-error"))
                    .max_w(px(ERROR_MAX_WIDTH))
                    .truncate()
                    .text_color(color::text_secondary())
                    .child(text.clone()),
            ),
            _ => None,
        })
        .child(div().debug_selector(|| button_id.to_owned()).child(button))
}

impl Render for DataPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.player.read(cx).view().accent;
        let format = self.format(cx);
        let page = cx.weak_entity();

        let export_button = {
            let page = page.clone();
            let running = self.export.is_running();
            TextButton::new(
                BUTTON_EXPORT,
                &self.focus_export,
                if running {
                    "Exporting\u{2026}"
                } else {
                    "Export\u{2026}"
                },
                accent,
            )
            .enabled(!running)
            .on_click(move |_, cx| {
                let _ = page.update(cx, |p, cx| p.start_export(cx));
            })
        };
        let report_button = {
            let page = page.clone();
            let running = self.report.is_running();
            TextButton::new(
                BUTTON_REPORT,
                &self.focus_report,
                if running {
                    "Creating\u{2026}"
                } else {
                    "Create\u{2026}"
                },
                accent,
            )
            .enabled(!running)
            .on_click(move |_, cx| {
                let _ = page.update(cx, |p, cx| p.start_report(cx));
            })
        };

        let export_row = SettingsRow::select(
            ROW_EXPORT,
            &self.focus_format,
            "Export library",
            format.label(),
            accent,
        )
        .action(control(&self.export, export_button, BUTTON_EXPORT, cx))
        .on_open(move |window, cx| {
            let _ = page.update(cx, |p, cx| p.open_formats(window, cx));
        });
        let report_row = SettingsRow::plain(ROW_REPORT, "Diagnostics report", accent)
            .action(control(&self.report, report_button, BUTTON_REPORT, cx));

        div()
            .flex()
            .flex_col()
            .w_full()
            .child(SettingsSection::new("Data").row(export_row).row(report_row))
    }
}
