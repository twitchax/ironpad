//! `/admin`, the single-operator panel (PRD-0063).
//!
//! Everything here is behind `crate::auth::admin_user`, enforced in the server
//! functions rather than in this component: a page can only decline to *draw*
//! something, and the data must not cross the wire in the first place.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use crate::components::app_layout::LayoutContext;
use crate::components::social_meta::mark_not_found;
use crate::server_fns::{
    admin_list_users, admin_overview, admin_revoke_user_sessions, admin_wipe_cache_tier,
};

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

    // Bumped after a revoke so the list refetches; the counts it shows are the
    // thing the action changes.
    let users_epoch = RwSignal::new(0_u32);
    let users = Resource::new(
        move || users_epoch.get(),
        |_| async move { admin_list_users().await },
    );

    // Bumped after a wipe so the sizes refetch.
    let overview_epoch = RwSignal::new(0_u32);
    let overview = Resource::new(
        move || overview_epoch.get(),
        |_| async move { admin_overview().await },
    );

    let wipe = move |tier: ironpad_common::CacheTier, name: String, size: String| {
        #[cfg(feature = "hydrate")]
        {
            // The confirm states the measured size and what is actually lost,
            // because "are you sure?" does not distinguish clearing 120MB of
            // scratch workspaces from throwing away every compiled cell.
            let consequence = if tier.valve_may_clear() {
                "Cells will recompile from scratch until it is rebuilt."
            } else {
                "Every compiled cell is discarded. Readers will wait for a \
                 cold compile on notebooks that are currently instant."
            };
            let confirmed = web_sys::window().is_some_and(|w: web_sys::Window| {
                w.confirm_with_message(&format!(
                    "Clear the {name} cache ({size})?\n\n{consequence}\n\n\
                     This cannot be undone."
                ))
                .unwrap_or(false)
            });
            if !confirmed {
                return;
            }
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = (&name, &size);

        leptos::task::spawn_local(async move {
            let _ = admin_wipe_cache_tier(tier).await;
            overview_epoch.update(|e| *e += 1);
        });
    };

    let revoke = move |github_id: String, login: String, sessions: u64| {
        // `web_sys` is a hydrate-only dependency here, and a click handler can
        // only run on the client anyway; SSR compiles this branch away.
        #[cfg(feature = "hydrate")]
        {
            let confirmed = web_sys::window().is_some_and(|w: web_sys::Window| {
                w.confirm_with_message(&format!(
                    "Sign {login} out of {sessions} session(s)?\n\n\
                     Their notebooks and shares are untouched, and they can \
                     sign back in."
                ))
                .unwrap_or(false)
            });
            if !confirmed {
                return;
            }
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = (&login, sessions);

        leptos::task::spawn_local(async move {
            let _ = admin_revoke_user_sessions(github_id).await;
            users_epoch.update(|e| *e += 1);
        });
    };

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
                            <table class="ironpad-admin-table ironpad-admin-table--cache">
                                <thead>
                                    <tr>
                                        <th scope="col">"Tier"</th>
                                        <th scope="col">"Size"</th>
                                        <th scope="col">"Cleared automatically"</th>
                                        <th scope="col">
                                            <span class="ironpad-visually-hidden">"Actions"</span>
                                        </th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {o.cache_tiers.into_iter().map(|t| {
                                        let size = human_bytes(t.bytes);
                                        let (tier, name, arg_size) =
                                            (t.tier, t.name.clone(), size.clone());
                                        let empty = t.bytes == 0;
                                        view! {
                                            <tr>
                                                <td>{t.name.clone()}</td>
                                                <td>{size}</td>
                                                <td>
                                                    {if t.valve_may_clear {
                                                        "under disk pressure"
                                                    } else {
                                                        "never"
                                                    }}
                                                </td>
                                                <td>
                                                    <button
                                                        class="ironpad-admin-action"
                                                        disabled=empty
                                                        title=if empty {
                                                            "Nothing to clear"
                                                        } else {
                                                            "Clear this tier now"
                                                        }
                                                        on:click=move |_| wipe(
                                                            tier,
                                                            name.clone(),
                                                            arg_size.clone(),
                                                        )
                                                    >
                                                        "Clear"
                                                    </button>
                                                </td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>

                            <h2>"Users"</h2>
                            <Suspense fallback=|| view! { <p>"Loading…"</p> }>
                                {move || Suspend::new(async move {
                                    match users.await {
                                        Ok(Some(list)) if list.is_empty() => {
                                            view! { <p>"No one has signed in yet."</p> }.into_any()
                                        }
                                        Ok(Some(list)) => view! {
                                            <table class="ironpad-admin-table ironpad-admin-table--users">
                                                <thead>
                                                    <tr>
                                                        <th scope="col">"User"</th>
                                                        <th scope="col">"Sessions"</th>
                                                        <th scope="col">"Published"</th>
                                                        <th scope="col">"Since"</th>
                                                        <th scope="col"><span class="ironpad-visually-hidden">"Actions"</span></th>
                                                    </tr>
                                                </thead>
                                                <tbody>
                                                    {list.into_iter().map(|u| {
                                                        let (id, login) = (u.github_id.clone(), u.login.clone());
                                                        let sessions = u.sessions;
                                                        let has_sessions = sessions > 0;
                                                        view! {
                                                            <tr>
                                                                <td>{u.login.clone()}</td>
                                                                <td>{sessions}</td>
                                                                <td>{u.owned_shares}</td>
                                                                <td>{u.created_at.chars().take(10).collect::<String>()}</td>
                                                                <td>
                                                                    <button
                                                                        class="ironpad-admin-action"
                                                                        disabled=!has_sessions
                                                                        title=if has_sessions {
                                                                            "Sign this user out everywhere"
                                                                        } else {
                                                                            "No active sessions"
                                                                        }
                                                                        on:click=move |_| revoke(id.clone(), login.clone(), sessions)
                                                                    >
                                                                        "Revoke sessions"
                                                                    </button>
                                                                </td>
                                                            </tr>
                                                        }
                                                    }).collect_view()}
                                                </tbody>
                                            </table>
                                        }.into_any(),
                                        // Denial cannot happen here: the page
                                        // only renders once the overview call
                                        // has already passed the same gate.
                                        Ok(None) => view! { <p>"Page not found."</p> }.into_any(),
                                        Err(e) => view! {
                                            <p class="ironpad-admin-error">
                                                "Could not list users: " {e.to_string()}
                                            </p>
                                        }.into_any(),
                                    }
                                })}
                            </Suspense>
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
