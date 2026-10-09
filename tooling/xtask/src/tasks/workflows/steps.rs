use gh_workflow::*;
use serde_json::Value;

use crate::tasks::workflows::{
    runners::Platform,
    steps::named::function_name,
    vars::{self, StepOutput},
};

pub(crate) fn use_clang(job: Job) -> Job {
    job.add_env(Env::new("CC", "clang"))
        .add_env(Env::new("CXX", "clang++"))
}

const SCCACHE_R2_BUCKET: &str = "sccache-zed";

pub(crate) const BASH_SHELL: &str = "bash -euxo pipefail {0}";
// https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#jobsjob_idstepsshell
pub const PWSH_SHELL: &str = "pwsh";

pub(crate) struct Nextest(Step<Run>);

pub(crate) fn cargo_nextest(platform: Platform) -> Nextest {
    Nextest(named::run(
        platform,
        "cargo nextest run --workspace --no-fail-fast --no-tests=warn",
    ))
}

impl Nextest {
    #[allow(dead_code)]
    pub(crate) fn with_filter_expr(mut self, filter_expr: &str) -> Self {
        if let Some(nextest_command) = self.0.value.run.as_mut() {
            nextest_command.push_str(&format!(r#" -E "{filter_expr}""#));
        }
        self
    }

    pub(crate) fn with_changed_packages_filter(mut self, orchestrate_job: &str) -> Self {
        if let Some(nextest_command) = self.0.value.run.as_mut() {
            nextest_command.push_str(&format!(
                r#"${{{{ needs.{orchestrate_job}.outputs.changed_packages && format(' -E "{{0}}"', needs.{orchestrate_job}.outputs.changed_packages) || '' }}}}"#
            ));
        }
        self
    }
}

impl From<Nextest> for Step<Run> {
    fn from(value: Nextest) -> Self {
        value.0
    }
}

#[derive(Default)]
enum FetchDepth {
    #[default]
    Shallow,
    Full,
    Custom(serde_json::Value),
}

#[derive(Default)]
pub(crate) struct CheckoutStep {
    fetch_depth: FetchDepth,
    token: Option<String>,
    ref_: Option<String>,
}

impl CheckoutStep {
    pub fn with_full_history(mut self) -> Self {
        self.fetch_depth = FetchDepth::Full;
        self
    }

    pub fn with_custom_fetch_depth(mut self, fetch_depth: impl Into<Value>) -> Self {
        self.fetch_depth = FetchDepth::Custom(fetch_depth.into());
        self
    }

    /// Sets `fetch-depth` to `2` on the main branch and `350` on all other branches.
    pub fn with_deep_history_on_non_main(self) -> Self {
        self.with_custom_fetch_depth("${{ github.ref == 'refs/heads/main' && 2 || 350 }}")
    }

    pub fn with_token(mut self, token: &StepOutput) -> Self {
        self.token = Some(token.to_string());
        self
    }

    pub fn with_ref(mut self, ref_: impl ToString) -> Self {
        self.ref_ = Some(ref_.to_string());
        self
    }
}

impl From<CheckoutStep> for Step<Use> {
    fn from(value: CheckoutStep) -> Self {
        Step::new("steps::checkout_repo".to_string())
            .uses(
                "actions",
                "checkout",
                "93cb6efe18208431cddfb8368fd83d5badbf9bfd", // v5.0.1
            )
            // prevent checkout action from running `git clean -ffdx` which
            // would delete the target directory
            .add_with(("clean", false))
            .map(|step| match value.fetch_depth {
                FetchDepth::Shallow => step,
                FetchDepth::Full => step.add_with(("fetch-depth", 0)),
                FetchDepth::Custom(depth) => step.add_with(("fetch-depth", depth)),
            })
            .when_some(value.ref_, |step, ref_| step.add_with(("ref", ref_)))
            .when_some(value.token, |step, token| step.add_with(("token", token)))
    }
}

impl FluentBuilder for CheckoutStep {}

pub fn checkout_repo() -> CheckoutStep {
    CheckoutStep::default()
}

// Audit mode: logs egress, blocks nothing. Namespace requires v2.19.0+.
pub fn harden_runner() -> Step<Use> {
    named::uses(
        "step-security",
        "harden-runner",
        "9af89fc71515a100421586dfdb3dc9c984fbf411", // v2.19.4
    )
    .add_with(("egress-policy", "audit"))
}

pub fn setup_pnpm() -> Step<Use> {
    named::uses(
        "pnpm",
        "action-setup",
        "fc06bc1257f339d1d5d8b3a19a8cae5388b55320", // v4.4.0
    )
    .add_with(("version", "9"))
}

pub fn setup_node() -> Step<Use> {
    named::uses(
        "actions",
        "setup-node",
        "48b55a011bda9f5d6aeb4c2d9c7362e8dae4041e", // v6.4
    )
    .add_with(("node-version", "24"))
    .add_with(("check-latest", true))
    .add_with(("package-manager-cache", false))
}

pub fn setup_sentry() -> Step<Use> {
    named::uses(
        "matbour",
        "setup-sentry-cli",
        "3e938c54b3018bdd019973689ef984e033b0454b",
    )
    .add_with(("token", vars::SENTRY_AUTH_TOKEN))
}

pub fn prettier() -> Step<Run> {
    named::bash("./script/prettier")
}

pub fn cargo_fmt() -> Step<Run> {
    named::bash("cargo fmt --all -- --check")
}

pub fn taiki_install_action(tool: &str) -> Step<Use> {
    Step::new(named::function_name(1))
        .uses(
            "taiki-e",
            "install-action",
            "a6b2e2dcd845ddd7f509ce4f3ed3d922b80cc5d9", // v2.84.0
        )
        .add_with(("tool", tool))
}

pub fn cargo_install_nextest() -> Step<Use> {
    taiki_install_action("nextest")
}

pub fn setup_cargo_config(platform: Platform) -> Step<Run> {
    match platform {
        Platform::Windows => named::pwsh(indoc::indoc! {r#"
            New-Item -ItemType Directory -Path "./../.cargo" -Force
            Copy-Item -Path "./.cargo/ci-config.toml" -Destination "./../.cargo/config.toml"
        "#}),

        Platform::Linux | Platform::Mac => named::bash(indoc::indoc! {r#"
            mkdir -p ./../.cargo
            cp ./.cargo/ci-config.toml ./../.cargo/config.toml
        "#}),
    }
}

pub fn cleanup_cargo_config(platform: Platform) -> Step<Run> {
    let step = match platform {
        Platform::Windows => named::pwsh(indoc::indoc! {r#"
            Remove-Item -Recurse -Path "./../.cargo" -Force -ErrorAction SilentlyContinue
        "#}),
        Platform::Linux | Platform::Mac => named::bash(indoc::indoc! {r#"
            rm -rf ./../.cargo
        "#}),
    };

    step.if_condition(Expression::new("always()"))
}

pub fn clear_target_dir_if_large(platform: Platform) -> Step<Run> {
    match platform {
        Platform::Windows => named::pwsh("./script/clear-target-dir-if-larger-than.ps1 350 200"),
        Platform::Linux => named::bash("./script/clear-target-dir-if-larger-than 350 200"),
        Platform::Mac => named::bash("./script/clear-target-dir-if-larger-than 350 200"),
    }
}

pub fn clippy(platform: Platform, target: Option<&str>) -> Step<Run> {
    match platform {
        Platform::Windows => named::pwsh("./script/clippy.ps1"),
        _ => match target {
            Some(target) => named::bash(format!("./script/clippy --target {target}")),
            None => named::bash("./script/clippy"),
        },
    }
}

pub fn install_rustup_target(target: &str) -> Step<Run> {
    named::bash(format!("rustup target add {target}"))
}

pub fn cache_rust_dependencies_namespace() -> Step<Use> {
    named::uses(
        "namespacelabs",
        "nscloud-cache-action",
        "a90bb5d4b27522ce881c6e98eebd7d7e6d1653f9", // v1
    )
    .add_with(("cache", "rust"))
    .add_with(("path", "~/.rustup"))
}

pub fn setup_sccache(platform: Platform) -> Step<Run> {
    let step = match platform {
        Platform::Windows => named::pwsh("./script/setup-sccache.ps1"),
        Platform::Linux | Platform::Mac => named::bash("./script/setup-sccache"),
    };
    step.add_env(("R2_ACCOUNT_ID", vars::R2_ACCOUNT_ID))
        .add_env(("R2_ACCESS_KEY_ID", vars::R2_ACCESS_KEY_ID))
        .add_env(("R2_SECRET_ACCESS_KEY", vars::R2_SECRET_ACCESS_KEY))
        .add_env(("SCCACHE_BUCKET", SCCACHE_R2_BUCKET))
}

pub fn show_sccache_stats(platform: Platform) -> Step<Run> {
    match platform {
        // Use $env:RUSTC_WRAPPER (absolute path) because GITHUB_PATH changes
        // don't take effect until the next step in PowerShell.
        // Check if RUSTC_WRAPPER is set first (it won't be for fork PRs without secrets).
        Platform::Windows => {
            named::pwsh("if ($env:RUSTC_WRAPPER) { & $env:RUSTC_WRAPPER --show-stats }; exit 0")
        }
        Platform::Linux | Platform::Mac => named::bash("sccache --show-stats || true"),
    }
}

pub fn cache_nix_dependencies_namespace() -> Step<Use> {
    named::uses(
        "namespacelabs",
        "nscloud-cache-action",
        "a90bb5d4b27522ce881c6e98eebd7d7e6d1653f9", // v1
    )
    .add_with(("cache", "nix"))
}

pub fn cache_nix_store_macos() -> Step<Use> {
    // On macOS, `/nix` is on a read-only root filesystem so nscloud's `cache: nix`
    // cannot mount or symlink there. Instead we cache a user-writable directory and
    // use nix-store --import/--export in separate steps to transfer store paths.
    named::uses(
        "namespacelabs",
        "nscloud-cache-action",
        "a90bb5d4b27522ce881c6e98eebd7d7e6d1653f9", // v1
    )
    .add_with(("path", "~/nix-cache"))
}

pub fn setup_linux() -> Step<Run> {
    named::bash("./script/linux")
}

pub(crate) fn download_wasi_sdk() -> Step<Run> {
    named::bash("./script/download-wasi-sdk")
}

pub(crate) fn install_linux_dependencies(job: Job) -> Job {
    job.add_step(setup_linux()).add_step(download_wasi_sdk())
}

pub fn script(name: &str) -> Step<Run> {
    if name.ends_with(".ps1") {
        Step::new(name).run(name).shell(PWSH_SHELL)
    } else {
        Step::new(name).run(name)
    }
}

pub struct NamedJob<J: JobType = RunJob> {
    pub name: String,
    pub job: Job<J>,
}

// impl NamedJob {
//     pub fn map(self, f: impl FnOnce(Job) -> Job) -> Self {
//         NamedJob {
//             name: self.name,
//             job: f(self.job),
//         }
//     }
// }

pub(crate) const DEFAULT_REPOSITORY_OWNER_GUARD: &str =
    "(github.repository_owner == 'zed-industries' || github.repository_owner == 'zed-extensions')";

pub fn repository_owner_guard_expression(trigger_always: bool) -> Expression {
    Expression::new(format!(
        "{}{}",
        DEFAULT_REPOSITORY_OWNER_GUARD,
        trigger_always.then_some(" && always()").unwrap_or_default()
    ))
}

pub trait CommonJobConditions: Sized {
    fn with_repository_owner_guard(self) -> Self;
}

impl CommonJobConditions for Job {
    fn with_repository_owner_guard(self) -> Self {
        self.cond(repository_owner_guard_expression(false))
    }
}

pub trait CommonPermissionSets: Sized {
    fn with_minimal_permissions(self) -> Self;
}

impl CommonPermissionSets for Workflow {
    fn with_minimal_permissions(self) -> Self {
        self.permissions(Permissions::default().contents(Level::Read))
    }
}

pub(crate) fn release_job(deps: &[&NamedJob]) -> Job {
    dependant_job(deps)
        .with_repository_owner_guard()
        .timeout_minutes(60u32)
}

pub(crate) fn dependant_job(deps: &[&NamedJob]) -> Job {
    let job = Job::default();
    if deps.len() > 0 {
        job.needs(deps.iter().map(|j| j.name.clone()).collect::<Vec<_>>())
    } else {
        job
    }
}

impl FluentBuilder for Job {}
impl FluentBuilder for Workflow {}
impl FluentBuilder for Input {}
impl<T> FluentBuilder for Step<T> {}

/// A helper trait for building complex objects with imperative conditionals in a fluent style.
/// Copied from GPUI to avoid adding GPUI as dependency
/// todo(ci) just put this in gh-workflow
#[allow(unused)]
pub trait FluentBuilder {
    /// Imperatively modify self with the given closure.
    fn map<U>(self, f: impl FnOnce(Self) -> U) -> U
    where
        Self: Sized,
    {
        f(self)
    }

    /// Conditionally modify self with the given closure.
    fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self
    where
        Self: Sized,
    {
        self.map(|this| if condition { then(this) } else { this })
    }

    /// Conditionally modify self with the given closure.
    fn when_else(
        self,
        condition: bool,
        then: impl FnOnce(Self) -> Self,
        else_fn: impl FnOnce(Self) -> Self,
    ) -> Self
    where
        Self: Sized,
    {
        self.map(|this| if condition { then(this) } else { else_fn(this) })
    }

    /// Conditionally unwrap and modify self with the given closure, if the given option is Some.
    fn when_some<T>(self, option: Option<T>, then: impl FnOnce(Self, T) -> Self) -> Self
    where
        Self: Sized,
    {
        self.map(|this| {
            if let Some(value) = option {
                then(this, value)
            } else {
                this
            }
        })
    }
    /// Conditionally unwrap and modify self with the given closure, if the given option is None.
    fn when_none<T>(self, option: &Option<T>, then: impl FnOnce(Self) -> Self) -> Self
    where
        Self: Sized,
    {
        self.map(|this| if option.is_some() { this } else { then(this) })
    }
}

// (janky) helper to generate steps with a name that corresponds
// to the name of the calling function.
pub mod named {
    use super::*;

    /// Returns a uses step with the same name as the enclosing function.
    /// (You shouldn't inline this function into the workflow definition, you must
    /// wrap it in a new function.)
    pub fn uses(owner: &str, repo: &str, ref_: &str) -> Step<Use> {
        Step::new(function_name(1)).uses(owner, repo, ref_)
    }

    /// Returns a bash-script step with the same name as the enclosing function.
    /// (You shouldn't inline this function into the workflow definition, you must
    /// wrap it in a new function.)
    pub fn bash(script: impl AsRef<str>) -> Step<Run> {
        Step::new(function_name(1)).run(script.as_ref())
    }

    /// Returns a pwsh-script step with the same name as the enclosing function.
    /// (You shouldn't inline this function into the workflow definition, you must
    /// wrap it in a new function.)
    pub fn pwsh(script: &str) -> Step<Run> {
        Step::new(function_name(1)).run(script).shell(PWSH_SHELL)
    }

    /// Runs the command in either powershell or bash, depending on platform.
    /// (You shouldn't inline this function into the workflow definition, you must
    /// wrap it in a new function.)
    pub fn run(platform: Platform, script: &str) -> Step<Run> {
        match platform {
            Platform::Windows => Step::new(function_name(1)).run(script).shell(PWSH_SHELL),
            Platform::Linux | Platform::Mac => Step::new(function_name(1)).run(script),
        }
    }

    /// Returns a Workflow with the same name as the enclosing module with default
    /// set for the running shell.
    pub fn workflow() -> Workflow {
        Workflow::default()
            .name(
                named::function_name(1)
                    .split("::")
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .skip(1)
                    .rev()
                    .collect::<Vec<_>>()
                    .join("::"),
            )
            .permissions(Permissions::default())
            .defaults(Defaults::default().run(RunDefaults::default().shell(BASH_SHELL)))
    }

    /// Returns a Job with the same name as the enclosing function.
    /// (note job names may not contain `::`)
    pub fn job<J: JobType>(job: Job<J>) -> NamedJob<J> {
        NamedJob {
            name: function_name(1).split("::").last().unwrap().to_owned(),
            job,
        }
    }

    /// Returns the function name N callers above in the stack
    /// (typically 1).
    /// This only works because xtask always runs debug builds.
    pub fn function_name(i: usize) -> String {
        let mut name = "<unknown>".to_string();
        let mut count = 0;
        backtrace::trace(|frame| {
            if count < i + 3 {
                count += 1;
                return true;
            }
            backtrace::resolve_frame(frame, |cb| {
                if let Some(s) = cb.name() {
                    name = s.to_string()
                }
            });
            false
        });

        name.split("::")
            .skip_while(|s| s != &"workflows")
            .skip(1)
            .collect::<Vec<_>>()
            .join("::")
    }
}

const UPLOAD_ARTIFACT_SHA: &str = "043fb46d1a93c77aae656e7c1c64a875d1fc6a0a"; // v7.0.1
const DOWNLOAD_ARTIFACT_SHA: &str = "3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c"; // v8.0.1

pub(crate) enum IfNoFilesFound {
    #[allow(unused)]
    Warn,
    Error,
}

impl IfNoFilesFound {
    fn as_str(&self) -> &'static str {
        match self {
            IfNoFilesFound::Warn => "warn",
            IfNoFilesFound::Error => "error",
        }
    }
}

#[derive(Default)]
pub(crate) struct UploadArtifactStep {
    name: String,
    artifact_name: String,
    path: String,
    if_no_files_found: Option<IfNoFilesFound>,
    if_condition: Option<Expression>,
    overwrite: bool,
}

impl UploadArtifactStep {
    pub fn if_no_files_found(mut self, behavior: IfNoFilesFound) -> Self {
        self.if_no_files_found = Some(behavior);
        self
    }

    pub fn if_condition(mut self, condition: Expression) -> Self {
        self.if_condition = Some(condition);
        self
    }

    pub fn overwrite(mut self, overwrite: bool) -> Self {
        self.overwrite = overwrite;
        self
    }
}

impl From<UploadArtifactStep> for Step<Use> {
    fn from(value: UploadArtifactStep) -> Self {
        Step::new(value.name)
            .uses("actions", "upload-artifact", UPLOAD_ARTIFACT_SHA)
            .add_with(("name", value.artifact_name))
            .add_with(("path", value.path))
            .when_some(value.if_no_files_found, |step, behavior| {
                step.add_with(("if-no-files-found", behavior.as_str()))
            })
            .when_some(value.if_condition, |step, condition| {
                step.if_condition(condition)
            })
            .when(value.overwrite, |step| step.add_with(("overwrite", true)))
    }
}

impl FluentBuilder for UploadArtifactStep {}

pub fn upload_artifact(
    artifact_name: impl Into<String>,
    path: impl Into<String>,
) -> UploadArtifactStep {
    UploadArtifactStep {
        name: function_name(1),
        artifact_name: artifact_name.into(),
        path: path.into(),
        ..Default::default()
    }
}

#[derive(Default)]
pub(crate) struct DownloadArtifactStep {
    name: String,
    path: Option<String>,
}

impl DownloadArtifactStep {
    pub fn path(mut self, path: &str) -> Self {
        self.path = Some(path.to_string());
        self
    }
}

impl From<DownloadArtifactStep> for Step<Use> {
    fn from(value: DownloadArtifactStep) -> Self {
        Step::new(value.name)
            .uses("actions", "download-artifact", DOWNLOAD_ARTIFACT_SHA)
            .when_some(value.path, |step, path| step.add_with(("path", path)))
    }
}

impl FluentBuilder for DownloadArtifactStep {}

pub fn download_artifact() -> DownloadArtifactStep {
    DownloadArtifactStep {
        name: function_name(1),
        ..Default::default()
    }
}

/// Non-exhaustive list of the permissions to be set for a GitHub app token.
///
/// See https://github.com/actions/create-github-app-token?tab=readme-ov-file#permission-permission-name
/// and beyond for a full list of available permissions.
#[allow(unused)]
pub(crate) enum TokenPermissions {
    Contents,
    Issues,
    Members,
    PullRequests,
    Workflows,
}

impl TokenPermissions {
    pub fn environment_name(&self) -> &'static str {
        match self {
            TokenPermissions::Contents => "permission-contents",
            TokenPermissions::Issues => "permission-issues",
            TokenPermissions::Members => "permission-members",
            TokenPermissions::PullRequests => "permission-pull-requests",
            TokenPermissions::Workflows => "permission-workflows",
        }
    }
}

pub(crate) struct Unset;

pub(crate) struct GenerateAppToken<'a, Target = Unset, Permissions = Unset> {
    job_name: String,
    app_id: &'a str,
    app_secret: &'a str,
    repository_target: Target,
    permissions: Permissions,
}

impl<'a, Permissions> GenerateAppToken<'a, Unset, Permissions> {
    pub fn for_repository(
        self,
        repository_target: RepositoryTarget,
    ) -> GenerateAppToken<'a, RepositoryTarget, Permissions> {
        GenerateAppToken {
            job_name: self.job_name,
            app_id: self.app_id,
            app_secret: self.app_secret,
            repository_target,
            permissions: self.permissions,
        }
    }
}

impl<'a, Target> GenerateAppToken<'a, Target, Unset> {
    pub fn with_permissions(
        self,
        permissions: impl Into<Vec<(TokenPermissions, Level)>>,
    ) -> GenerateAppToken<'a, Target, Vec<(TokenPermissions, Level)>> {
        GenerateAppToken {
            job_name: self.job_name,
            app_id: self.app_id,
            app_secret: self.app_secret,
            repository_target: self.repository_target,
            permissions: permissions.into(),
        }
    }
}

impl<'a> From<GenerateAppToken<'a, RepositoryTarget, Vec<(TokenPermissions, Level)>>>
    for (Step<Use>, StepOutput)
{
    fn from(token: GenerateAppToken<'a, RepositoryTarget, Vec<(TokenPermissions, Level)>>) -> Self {
        let input = token.permissions.into_iter().fold(
            Input::default()
                .add("app-id", token.app_id)
                .add("private-key", token.app_secret)
                .add("owner", token.repository_target.owner)
                .add("repositories", token.repository_target.repositories),
            |input, (permission, level)| {
                input.add(
                    permission.environment_name(),
                    serde_json::to_value(&level).unwrap_or_default(),
                )
            },
        );
        let step = Step::new(token.job_name)
            .uses(
                "actions",
                "create-github-app-token",
                "f8d387b68d61c58ab83c6c016672934102569859",
            )
            .id("generate-token")
            .add_with(input);

        let generated_token = StepOutput::new(&step, "token");
        (step, generated_token)
    }
}

pub(crate) struct RepositoryTarget {
    owner: String,
    repositories: String,
}

impl RepositoryTarget {
    pub fn new<T: ToString>(owner: T, repositories: &[&str]) -> Self {
        Self {
            owner: owner.to_string(),
            repositories: repositories.join("\n"),
        }
    }

    pub fn current() -> Self {
        Self::new(
            "${{ github.repository_owner }}",
            &["${{ github.event.repository.name }}"],
        )
    }
}

pub fn authenticate_as_zippy() -> GenerateAppToken<'static> {
    generate_token_with_job_name(vars::ZED_ZIPPY_APP_ID, vars::ZED_ZIPPY_APP_PRIVATE_KEY)
}

fn generate_token_with_job_name<'a>(
    app_id_source: &'a str,
    app_secret_source: &'a str,
) -> GenerateAppToken<'a> {
    GenerateAppToken {
        job_name: function_name(1),
        app_id: app_id_source,
        app_secret: app_secret_source,
        repository_target: Unset,
        permissions: Unset,
    }
}
