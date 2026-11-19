//! Provider choices shared by onboarding and repository connection actions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GitProvider {
    #[default]
    Github,
    Gitlab,
}

impl GitProvider {
    pub const ALL: [Self; 2] = [Self::Github, Self::Gitlab];

    pub fn label(self) -> &'static str {
        match self {
            Self::Github => "GitHub",
            Self::Gitlab => "GitLab",
        }
    }

    pub fn cli(self) -> &'static str {
        match self {
            Self::Github => "gh",
            Self::Gitlab => "glab",
        }
    }

    pub fn status_args(self) -> &'static [&'static str] {
        match self {
            Self::Github => &["auth", "status", "--hostname", "github.com"],
            // Outside a repository, glab uses the host selected during login.
            Self::Gitlab => &["auth", "status"],
        }
    }

    pub fn login_command(self) -> &'static str {
        match self {
            Self::Github => "gh auth login --hostname github.com --git-protocol https --web",
            // Keep the hostname and login-method prompts: self-managed users
            // must be able to select their instance and its supported method.
            Self::Gitlab => "glab auth login",
        }
    }

    pub fn winget_package(self) -> &'static str {
        match self {
            Self::Github => "GitHub.cli",
            Self::Gitlab => "GLab.GLab",
        }
    }

    pub fn install_url(self) -> &'static str {
        match self {
            Self::Github => "https://cli.github.com/",
            Self::Gitlab => "https://docs.gitlab.com/cli/",
        }
    }
}
