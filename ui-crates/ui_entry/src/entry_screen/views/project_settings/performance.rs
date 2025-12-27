use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, Icon, IconName, divider::Divider, ActiveTheme as _,
};
use super::{types::{ProjectSettings, format_size}, helpers::render_info_section};
use crate::entry_screen::EntryScreen;

pub fn render_performance_tab(settings: &ProjectSettings, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let project_size = settings.disk_size.unwrap_or(0);
    let git_size = settings.git_repo_size.unwrap_or(0);
    
    // Calculate repository health score
    let health_score = calculate_repo_health(settings);
    let health_color = if health_score >= 80.0 {
        theme.accent
    } else if health_score >= 50.0 {
        theme.warning
    } else {
        theme.danger
    };
    
    v_flex()
        .gap_6()
        .child(
            div()
                .text_2xl()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(theme.foreground)
                .child("Performance & Optimization")
        )
        .child(Divider::horizontal())
        .child(
            h_flex()
                .gap_6()
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .p_4()
                        .border_1()
                        .border_color(theme.border)
                        .rounded_lg()
                        .bg(theme.sidebar)
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Repository Health")
                        )
                        .child(
                            div()
                                .text_3xl()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(health_color)
                                .child(format!("{:.0}%", health_score))
                        )
                        .child(
                            div()
                                .w_full()
                                .h(px(6.))
                                .bg(theme.border)
                                .rounded_full()
                                .child(
                                    div()
                                        .w(relative(health_score / 100.0))
                                        .h_full()
                                        .bg(health_color)
                                        .rounded_full()
                                )
                        )
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .p_4()
                        .border_1()
                        .border_color(theme.border)
                        .rounded_lg()
                        .bg(theme.sidebar)
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Total Commits")
                        )
                        .child(
                            div()
                                .text_3xl()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(theme.primary)
                                .child(settings.commit_count.map(|c| c.to_string()).unwrap_or_else(|| "0".to_string()))
                        )
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .p_4()
                        .border_1()
                        .border_color(theme.border)
                        .rounded_lg()
                        .bg(theme.sidebar)
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Disk Usage")
                        )
                        .child(
                            div()
                                .text_3xl()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(theme.accent)
                                .child(format_size(Some(project_size)))
                        )
                )
        )
        .child(render_info_section("Repository Statistics", vec![
            ("Total Commits", settings.commit_count.map(|c| c.to_string()).unwrap_or_else(|| "N/A".to_string())),
            ("Total Branches", settings.branch_count.map(|c| c.to_string()).unwrap_or_else(|| "N/A".to_string())),
            ("Git Repository Size", format_size(Some(git_size))),
            ("Git Size Ratio", if project_size > 0 {
                format!("{:.1}%", (git_size as f64 / project_size as f64) * 100.0)
            } else {
                "N/A".to_string()
            }),
        ], &theme))
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .mb_2()
                        .child("Optimization Recommendations")
                )
                .children(generate_optimization_recommendations(settings, &theme))
        )
        .child(
            v_flex()
                .gap_3()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .mb_2()
                        .child("Optimization Actions")
                )
                .child(
                    h_flex()
                        .gap_3()
                        .child(
                            Button::new("run-gc")
                                .label("Run Git GC")
                                .icon(IconName::Activity)
                                .flex_1()
                                .with_variant(ui::button::ButtonVariant::Primary)
                                .on_click({
                                    let path = settings.project_path.clone();
                                    move |_, _, _| {
                                        let _ = std::process::Command::new("git")
                                            .args(&["gc", "--aggressive"])
                                            .current_dir(&path)
                                            .spawn();
                                    }
                                })
                        )
                        .child(
                            Button::new("prune-now")
                                .label("Prune Objects")
                                .icon(IconName::Trash)
                                .flex_1()
                                .with_variant(ui::button::ButtonVariant::Secondary)
                                .on_click({
                                    let path = settings.project_path.clone();
                                    move |_, _, _| {
                                        let _ = std::process::Command::new("git")
                                            .args(&["prune", "--expire=now"])
                                            .current_dir(&path)
                                            .spawn();
                                    }
                                })
                        )
                        .child(
                            Button::new("clean-untracked")
                                .label("Clean Untracked")
                                .icon(IconName::Trash)
                                .flex_1()
                                .with_variant(ui::button::ButtonVariant::Secondary)
                                .on_click({
                                    let path = settings.project_path.clone();
                                    move |_, _, _| {
                                        let _ = std::process::Command::new("git")
                                            .args(&["clean", "-fd"])
                                            .current_dir(&path)
                                            .spawn();
                                    }
                                })
                        )
                )
        )
}

fn calculate_repo_health(settings: &ProjectSettings) -> f32 {
    let mut score: f32 = 100.0;
    
    // Penalize for large git size ratio
    if let (Some(git_size), Some(project_size)) = (settings.git_repo_size, settings.disk_size) {
        if project_size > 0 {
            let ratio = (git_size as f64 / project_size as f64) * 100.0;
            if ratio > 50.0 {
                score -= 20.0; // Large git size
            } else if ratio > 30.0 {
                score -= 10.0;
            }
        }
    }
    
    // Penalize for uncommitted changes
    if let Some(changes) = settings.uncommitted_changes {
        if changes > 50 {
            score -= 20.0; // Too many uncommitted changes
        } else if changes > 20 {
            score -= 10.0;
        }
    }
    
    // Bonus for having CI/CD
    if !settings.workflow_files.is_empty() {
        score += 10.0;
    }
    
    // Bonus for recent activity
    if settings.last_commit_date.is_some() {
        score += 5.0;
    }
    
    score.max(0.0_f32).min(100.0)
}

fn generate_optimization_recommendations(settings: &ProjectSettings, theme: &ui::theme::Theme) -> Vec<gpui::AnyElement> {
    let mut recommendations = Vec::new();
    
    // Check git size ratio
    if let (Some(git_size), Some(project_size)) = (settings.git_repo_size, settings.disk_size) {
        if project_size > 0 {
            let ratio = (git_size as f64 / project_size as f64) * 100.0;
            if ratio > 30.0 {
                recommendations.push(
                    render_recommendation_card(
                        "Large Git Repository",
                        &format!("Your .git folder is {:.1}% of total project size. Consider running 'git gc' to compress the repository.", ratio),
                        "high",
                        theme,
                    )
                );
            }
        }
    }
    
    // Check uncommitted changes
    if let Some(changes) = settings.uncommitted_changes {
        if changes > 20 {
            recommendations.push(
                render_recommendation_card(
                    "Many Uncommitted Changes",
                    &format!("You have {} uncommitted file(s). Consider committing or stashing your changes.", changes),
                    "medium",
                    theme,
                )
            );
        }
    }
    
    // Check for CI/CD
    if settings.workflow_files.is_empty() && settings.remote_url.is_some() {
        recommendations.push(
            render_recommendation_card(
                "No CI/CD Configuration",
                "Consider adding GitHub Actions workflows to automate builds and tests.",
                "low",
                theme,
            )
        );
    }
    
    // General recommendations
    recommendations.push(
        render_recommendation_card(
            "Use .gitignore",
            "Ensure build artifacts and dependencies are excluded from version control.",
            "info",
            theme,
        )
    );
    
    recommendations.push(
        render_recommendation_card(
            "Consider Git LFS",
            "For large binary assets (textures, models), use Git Large File Storage to reduce repository size.",
            "info",
            theme,
        )
    );
    
    if recommendations.is_empty() {
        recommendations.push(
            render_recommendation_card(
                "Repository Optimized",
                "Your repository is in good shape! No major optimizations needed.",
                "success",
                theme,
            )
        );
    }
    
    recommendations
}

fn render_recommendation_card(title: &str, desc: &str, severity: &str, theme: &ui::theme::Theme) -> gpui::AnyElement {
    
    
    let (bg_color, border_color, icon_color) = match severity {
        "high" => (
            theme.danger.opacity(0.1),
            theme.danger.opacity(0.3),
            theme.danger,
        ),
        "medium" => (
            theme.warning.opacity(0.1),
            theme.warning.opacity(0.3),
            theme.warning,
        ),
        "low" | "info" => (
            theme.accent.opacity(0.1),
            theme.accent.opacity(0.3),
            theme.accent,
        ),
        "success" => (
            theme.primary.opacity(0.1),
            theme.primary.opacity(0.3),
            theme.primary,
        ),
        _ => (
            theme.muted.opacity(0.1),
            theme.muted.opacity(0.3),
            theme.muted_foreground,
        ),
    };
    
    h_flex()
        .gap_3()
        .p_3()
        .rounded_lg()
        .bg(bg_color)
        .border_1()
        .border_color(border_color)
        .child(
            Icon::new(IconName::Activity)
                .size(px(20.))
                .text_color(icon_color)
        )
        .child(
            v_flex()
                .flex_1()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(title.to_string())
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(desc.to_string())
                )
        )
        .into_any_element()
}
