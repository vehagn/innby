//! The Users tab, for admins: the password users the admin UI makes, beside the config's,
//! which it shows but can't change. An admin gives each a role, or none, and a first
//! password, and later a new role, a new password, or deletes them; a new password or a
//! deletion logs them out everywhere (see `auth`). An admin can't change or delete
//! themselves here, so they can't lock themselves out.

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    response::Response,
};
use serde::Deserialize;

use super::{
    forms::{DeleteForm, SavedQuery},
    lang,
    page::{AdminState, PageError, redirect, saved},
    role::{Host, Role},
};
use crate::{
    config::{MAX_USER_LEN, valid_user_name},
    db::admin_user::{self, AdminUser},
    http::{render_form, server_error},
    i18n::{fill, filled},
    paths,
};

/// The shortest and longest password the admin UI sets.
const PASSWORD_LEN: std::ops::RangeInclusive<usize> = 10..=200;

/// The roles a user can have, as the forms name them: `none` for none of their own.
const ROLES: [&str; 4] = ["admin", "editor", "viewer", "none"];

/// A user as the pages show them.
pub(super) struct UserRow {
    pub(super) name: String,
    pub(super) role: &'static str,
    pub(super) path: String,
}

impl UserRow {
    fn new(name: &str, role: Option<Role>) -> Self {
        Self {
            name: name.to_owned(),
            role: role.map_or("none", Role::name),
            path: paths::admin(&paths::path(paths::USER, name)),
        }
    }
}

/// A role in a form: its code, its name, and whether it's ticked.
pub(super) struct RoleChoice {
    pub(super) code: &'static str,
    pub(super) name: &'static str,
    pub(super) checked: bool,
}

fn role_choices(current: &str) -> Vec<RoleChoice> {
    ROLES
        .iter()
        .map(|&code| RoleChoice {
            code,
            name: role_name(code),
            checked: code == current,
        })
        .collect()
}

/// A role's name in the page's language.
pub fn role_name(role: &str) -> &'static str {
    let text = lang::text();
    match role {
        "admin" => &text.role_admin,
        "editor" => &text.role_editor,
        "viewer" => &text.role_viewer,
        _ => &text.role_none,
    }
}

#[derive(Template)]
#[template(path = "admin/users.html")]
struct UsersPage {
    users: Vec<UserRow>,
    from_config: Vec<UserRow>,
    /// The new user's form, kept when it needs fixing.
    name: String,
    roles: Vec<RoleChoice>,
    notice: Option<String>,
    error: Option<String>,
}

async fn users_page(
    state: &AdminState,
    form: NewUser,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, PageError> {
    let users = admin_user::list(&state.db).await?;
    let needs_fixing = error.is_some();
    let page = UsersPage {
        users: users
            .iter()
            .map(|user| UserRow::new(&user.name, stored_role(user)))
            .collect(),
        from_config: state
            .auth
            .config_users()
            .iter()
            .map(|login| UserRow::new(&login.user, login.role))
            .collect(),
        roles: role_choices(&form.role),
        name: form.name,
        notice,
        error,
    };
    Ok(render_form(page, needs_fixing))
}

fn stored_role(user: &AdminUser) -> Option<Role> {
    user.role.as_deref().and_then(Role::from_name)
}

#[derive(Deserialize)]
pub(super) struct NewUser {
    name: String,
    role: String,
    #[serde(default)]
    password: String,
}

/// The role as stored, or what's wrong with it.
fn role_from(form: &str) -> Result<Option<&'static str>, String> {
    match form {
        "none" => Ok(None),
        role => Role::from_name(role)
            .map(|role| Some(role.name()))
            .ok_or_else(|| lang::text().error_role.clone()),
    }
}

fn check_password(password: &str) -> Result<(), String> {
    if PASSWORD_LEN.contains(&password.chars().count()) {
        Ok(())
    } else {
        let text = &lang::text().error_password;
        Err(fill(
            text,
            &[("min", PASSWORD_LEN.start()), ("max", PASSWORD_LEN.end())],
        ))
    }
}

/// The users, and the form for a new one.
pub(super) async fn list(
    State(state): State<AdminState>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let blank = NewUser {
        name: String::new(),
        role: "editor".into(),
        password: String::new(),
    };
    let notice = query.saved.map(|_| lang::text().saved.clone());
    users_page(&state, blank, notice, None).await
}

/// Makes a user, with a role and a first password.
pub(super) async fn create(
    State(state): State<AdminState>,
    Form(form): Form<NewUser>,
) -> Result<Response, PageError> {
    let text = lang::text();
    let name = form.name.trim().to_owned();
    let checked = if !valid_user_name(&name) {
        Err(fill(&text.error_new_user_name, &[("max", &MAX_USER_LEN)]))
    } else if state
        .auth
        .config_users()
        .iter()
        .any(|login| login.user == name)
    {
        Err(filled(&text.error_user_taken, "name", &name))
    } else {
        role_from(&form.role).and_then(|role| check_password(&form.password).map(|()| role))
    };
    let role = match checked {
        Ok(role) => role,
        Err(error) => return users_page(&state, form, None, Some(error)).await,
    };
    let hash = match state.auth.hash(form.password.clone()).await {
        Ok(hash) => hash,
        Err(err) => return Ok(server_error(err)),
    };
    if !admin_user::create(&state.db, &name, role, &hash).await? {
        let error = filled(&text.error_user_taken, "name", &name);
        return users_page(&state, form, None, Some(error)).await;
    }
    tracing::info!("admin user created");
    Ok(saved(paths::USER, &name))
}

#[derive(Template)]
#[template(path = "admin/user.html")]
struct UserPage {
    user: UserRow,
    roles: Vec<RoleChoice>,
    /// Whether it's the admin looking, who can't change themselves here.
    yourself: bool,
    links: UserLinks,
    notice: Option<String>,
    error: Option<String>,
}

struct UserLinks {
    page: String,
    delete: String,
}

async fn user_page(
    state: &AdminState,
    name: &str,
    notice: Option<String>,
    error: Option<String>,
) -> Result<Response, PageError> {
    let user = admin_user::find(&state.db, name)
        .await?
        .ok_or(PageError::Missing)?;
    let needs_fixing = error.is_some();
    let row = UserRow::new(&user.name, stored_role(&user));
    let page = UserPage {
        roles: role_choices(row.role),
        user: row,
        yourself: *Host::current().user == user.name,
        links: UserLinks {
            page: paths::admin(&paths::path(paths::USER, name)),
            delete: paths::admin(&paths::path(paths::USER_DELETE, name)),
        },
        notice,
        error,
    };
    Ok(render_form(page, needs_fixing))
}

pub(super) async fn show(
    State(state): State<AdminState>,
    Path(name): Path<String>,
    Query(query): Query<SavedQuery>,
) -> Result<Response, PageError> {
    let notice = query.saved.map(|_| lang::text().saved.clone());
    user_page(&state, &name, notice, None).await
}

/// A user's new role, or new password: whichever the form sends.
#[derive(Deserialize)]
pub(super) struct EditUser {
    role: Option<String>,
    password: Option<String>,
}

pub(super) async fn edit(
    State(state): State<AdminState>,
    Path(name): Path<String>,
    Form(form): Form<EditUser>,
) -> Result<Response, PageError> {
    if *Host::current().user == name {
        let error = lang::text().error_yourself.clone();
        return user_page(&state, &name, None, Some(error)).await;
    }
    let changed = match (form.role, form.password) {
        (Some(role), _) => match role_from(&role) {
            Ok(role) => admin_user::set_role(&state.db, &name, role).await?,
            Err(error) => return user_page(&state, &name, None, Some(error)).await,
        },
        (None, Some(password)) => {
            if let Err(error) = check_password(&password) {
                return user_page(&state, &name, None, Some(error)).await;
            }
            let hash = match state.auth.hash(password).await {
                Ok(hash) => hash,
                Err(err) => return Ok(server_error(err)),
            };
            admin_user::set_password(&state.db, &name, &hash).await?
        }
        (None, None) => return user_page(&state, &name, None, None).await,
    };
    if !changed {
        return Err(PageError::Missing);
    }
    tracing::info!("admin user changed");
    Ok(saved(paths::USER, &name))
}

pub(super) async fn delete(
    State(state): State<AdminState>,
    Path(name): Path<String>,
    Form(form): Form<DeleteForm>,
) -> Result<Response, PageError> {
    let text = lang::text();
    if *Host::current().user == name {
        return user_page(&state, &name, None, Some(text.error_yourself.clone())).await;
    }
    if form.confirm.is_none() {
        let error = text.error_confirm_delete.clone();
        return user_page(&state, &name, None, Some(error)).await;
    }
    if !admin_user::delete(&state.db, &name).await? {
        return Err(PageError::Missing);
    }
    tracing::info!("admin user deleted");
    Ok(redirect(&format!("{}?saved", paths::USERS)))
}
