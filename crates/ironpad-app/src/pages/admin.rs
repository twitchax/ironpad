//! `/admin`, the single-operator panel (PRD-0063).
//!
//! Everything here is behind `crate::auth::admin_user`, enforced in the server
//! functions rather than in this component: a page can only decline to *draw*
//! something, and the data must not cross the wire in the first place.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use crate::components::app_layout::LayoutContext;
use crate::components::social_meta::mark_not_found;
use crate::server_fns::admin_overview;

/// Human-readable byte size. Whole units below 10 keep one decimal, so 3.2GB
/// does not read as 3GB.
///
/// Integer arithmetic throughout: `bytes as f64` is a precision loss clippy
/// rejects under pedantic, and an `#[allow]` would be papering over a real (if
/// distant) inaccuracy when the exact version is no harder to read.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    let mut divisor: u64 = 1;
    let mut unit = 0;
    while bytes / divisor >= 1024 && unit < UNITS.len() - 1 {
        divisor *= 1024;
        unit += 1;
    }

    if unit == 0 {
        return format!("{bytes} B");
    }

    let mut whole = bytes / divisor;
    // Rounded, not truncated: 3,435,973,836 bytes is 3.19999 GB, and reading
    // that as "3.1 GB" is further from the truth than "3.2 GB".
    let mut tenths = ((bytes % divisor) * 10 + divisor / 2) / divisor;
    if tenths == 10 {
        whole += 1;
        tenths = 0;
    }
    // The carry can push a value past its own unit: one byte under 1MB rounds
    // to 1024.0 KB, which should read as 1.0 MB.
    if whole >= 1024 && unit < UNITS.len() - 1 {
        whole /= 1024;
        unit += 1;
    }

    if whole >= 10 {
        format!("{whole} {}", UNITS[unit])
    } else {
        format!("{whole}.{tenths} {}", UNITS[unit])
    }
}

/// Route component for `/admin`.
///
/// A non-admin gets the same "Page not found." the router's fallback renders,
/// with a real 404 status. A distinct "forbidden" would confirm to a prober
/// that the panel exists and that they have the right URL, which is the one
/// thing the gate is meant to withhold.
#[component]
pub fn AdminPage() -> impl IntoView {
    let ctx = expect_context::<LayoutContext>();
    ctx.notebook_title.set(None);

    let overview = Resource::new(|| (), |()| async move { admin_overview().await });

    view! {
        <Title text="Admin · ironpad"/>
        // Never indexed. robots.txt also disallows it, which is safe here in a
        // way it is not for /shared and /mutable: those need unfurlers to fetch
        // them for link previews, and several unfurlers honour robots.txt.
        // Nothing ever previews /admin.
        <Meta name="robots" content="noindex, nofollow"/>

        <Suspense fallback=|| view! { <p class="ironpad-admin-loading">"Loading…"</p> }>
            {move || Suspend::new(async move {
                match overview.await {
                    // Denied. Same body and status as any unknown route: a
                    // distinct "forbidden" would confirm the panel is real.
                    Ok(None) => {
                        mark_not_found();
                        view! { <p>"Page not found."</p> }.into_any()
                    }
                    // Only reachable once the gate has already passed, so this
                    // can say what went wrong. Reporting it as not-found made
                    // a broken database look like a permissions problem to the
                    // one person able to fix it.
                    Err(e) => view! {
                        <div class="ironpad-admin">
                            <h1>"Instance"</h1>
                            <p class="ironpad-admin-error">
                                "Could not read instance state: " {e.to_string()}
                            </p>
                        </div>
                    }.into_any(),
                    Ok(Some(o)) => view! {
                        <div class="ironpad-admin">
                            <h1>"Instance"</h1>
                            <dl class="ironpad-admin-stats">
                                <div><dt>"Users"</dt><dd>{o.users}</dd></div>
                                <div><dt>"Sessions"</dt><dd>{o.sessions}</dd></div>
                                <div><dt>"Published notebooks"</dt><dd>{o.mutable_shares}</dd></div>
                                <div>
                                    <dt>"Accounts database"</dt>
                                    <dd>{human_bytes(o.database_bytes)}</dd>
                                </div>
                            </dl>

                            <h2>"Compile cache"</h2>
                            <table class="ironpad-admin-table">
                                <thead>
                                    <tr>
                                        <th scope="col">"Tier"</th>
                                        <th scope="col">"Size"</th>
                                        <th scope="col">"Cleared automatically"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {o.cache_tiers.into_iter().map(|t| view! {
                                        <tr>
                                            <td>{t.name}</td>
                                            <td>{human_bytes(t.bytes)}</td>
                                            <td>
                                                {if t.valve_may_clear {
                                                    "under disk pressure"
                                                } else {
                                                    "never"
                                                }}
                                            </td>
                                        </tr>
                                    }).collect_view()}
                                </tbody>
                            </table>
                        </div>
                    }.into_any(),
                }
            })}
        </Suspense>
    }
}

#[cfg(test)]
mod tests {
    use super::human_bytes;

    #[test]
    fn bytes_render_at_a_readable_scale() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        // Below 10 of a unit keeps one decimal, so 3.2GB does not read as 3GB.
        assert_eq!(human_bytes(3_435_973_836), "3.2 GB");
        // At or above 10, the decimal is noise.
        assert_eq!(human_bytes(166 * 1024 * 1024), "166 MB");
    }

    #[test]
    fn units_step_at_the_boundary() {
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
    }

    #[test]
    fn rounding_carries_into_the_next_unit() {
        // 1048575 is one byte under 1MB and rounds to 1024.0 KB, which must
        // not print as "1024.0 KB" or "0.0 MB".
        assert_eq!(human_bytes(1024 * 1024 - 1), "1.0 MB");
        // Exactly one unit boundary stays put.
        assert_eq!(human_bytes(1024 * 1024), "1.0 MB");
    }
}
