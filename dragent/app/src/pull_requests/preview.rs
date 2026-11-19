use super::*;
/// The templates the preview's project keeps.
pub(super) fn templates() -> Vec<PrTemplate> {
    vec![
        PrTemplate {
            name: "Default".into(),
            body: "## Summary\n<!-- What changed and why -->\n\n## Changes\n\n## Testing\n- [ ] Unit tests\n- [ ] Manual check".into(),
            source: "repository".into(),
        },
        PrTemplate {
            name: "release".into(),
            body: "## Release notes\n\n## Rollback plan\n".into(),
            source: "organisation".into(),
        },
    ]
}

pub(crate) fn seed(
    v: &mut PullRequestsView,
    scene: usize,
    ctx: &mut ViewContext<PullRequestsView>,
) {
    v.preview = true;
    v.loading = false;
    v.detail_loading = false;
    v.errors.clear();
    v.project = None;
    v.filter = Filter::All;
    v.form_head.clear();
    v.form_current.clear();
    v.local_branches.clear();
    v.comparison = None;
    v.comparing = false;
    v.templates.clear();
    v.template = None;
    v.description_preview = false;
    v.describing = false;
    v.describe_error = None;
    v.body_undo = None;
    v.form_is_gitlab = false;
    v.state = "all".into();
    v.picker = None;
    v.base_query.clear();
    v.viewed.clear();
    v.list_only = false;
    v.extend_line = false;
    v.commit = None;
    v.commit_files = None;
    v.image_preview = false;
    v.review_drafts.clear();
    v.create_draft = None;
    v.description.clear();
    for editor in [&v.composer, &v.title_editor, &v.description_editor, &v.base_search] {
        ctx.update_view(editor, |e, c| e.set_text("", c));
    }

    v.pending.clear();
    v.line = None;
    v.split = false;
    v.reply = None;
    v.body.clear();
    v.outdated_comments.clear();
    v.repository = json!({"viewerPermission":"ADMIN","squashMergeAllowed":true,"mergeCommitAllowed":true,"rebaseMergeAllowed":true});
    v.theme = if scene == 5 {
        Theme::light()
    } else {
        Theme::default()
    };
    v.set_theme(v.theme, ctx);
    v.width.set(if scene == 12 || scene == 33 {
        760.0
    } else if scene == 7 {
        1000.0
    } else {
        1600.0
    });
    v.form = None;
    v.confirmation = None;
    v.tab = match scene {
        1 | 9 | 10 | 12 | 15 => Tab::Files,
        2 | 17 => Tab::Checks,
        3 => Tab::Activity,
        _ => Tab::Overview,
    };
    let target = Target {
        root: PathBuf::from("/demo/acme"),
        label: "Acme".into(),
    };
    v.targets = vec![target.clone()];
    let pr = json!({"number":128,"id":"PR_demo","title":"Add secure team invitations","body":"## What changed\nInvite teammates by email, with expiring single-use tokens and clear delivery status.\n\n### Review focus\n- Token hashing and expiry\n- Organization boundaries\n- Accessible error feedback\n\nCloses #117.\n\n```ts\nawait inviteTeammate(email, organizationId);\n```","url":"https://github.com/example/acme/pull/128","state":"OPEN","isDraft":false,"author":{"login":"ada"},"baseRefName":"main","headRefName":"feat/team-invites","headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","updatedAt":"2026-10-25T14:32:00Z","additions":186,"deletions":24,"changedFiles":3,"reviewDecision":"REVIEW_REQUIRED","mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED","statusCheckRollup":[{"name":"TypeScript","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://github.com/example/acme/actions/runs/1001"},{"name":"Integration tests","status":"COMPLETED","conclusion":"FAILURE","detailsUrl":"https://github.com/example/acme/actions/runs/1002"},{"name":"Accessibility","status":"IN_PROGRESS","conclusion":"","detailsUrl":"https://github.com/example/acme/actions/runs/1003"}],"labels":[{"name":"feature"},{"name":"security"}],"assignees":[{"login":"ada"}],"reviewRequests":[{"login":"sam"}],"commits":[{"oid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","messageHeadline":"Hash invitation tokens and enforce organization boundaries","committedDate":"2026-10-25T12:10:00Z"},{"oid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","messageHeadline":"Add expiry validation and accessible invitation feedback","committedDate":"2026-10-25T14:32:00Z"}],"comments":[{"author":{"login":"ada"},"body":"Ready for review. Please look closely at organization scoping and expired invitation retries.","createdAt":"2026-10-25T14:35:00Z"}],"reviews":[{"author":{"login":"sam"},"state":"COMMENTED","body":"The API structure looks good. One edge case below before this is ready."}]});
    v.items = vec![Item {
        target: target.clone(),
        data: pr.clone(),
    }];
    for (n, title, status) in [
        (127, "Improve command palette keyboard navigation", "ready"),
        (126, "Preserve remote workspace connection state", "draft"),
        (124, "Reduce editor startup work", "merged"),
    ] {
        let mut p = pr.clone();
        p["number"] = json!(n);
        p["title"] = json!(title);
        p["url"] = json!(format!("https://github.com/example/acme/pull/{n}"));
        p["headRefName"] = json!(format!("feat/change-{n}"));
        p["statusCheckRollup"] = json!([]);
        p["reviewDecision"] = json!("APPROVED");
        p["mergeStateStatus"] = json!("CLEAN");
        p["isDraft"] = json!(status == "draft");
        if status == "merged" {
            p["state"] = json!("MERGED");
        }
        v.items.push(Item {
            target: target.clone(),
            data: p,
        });
    }
    v.detail = v.items.first().cloned();
    v.selected = v.detail.as_ref().map(Item::key);
    v.pending_head = "a".repeat(40);
    v.files = vec![
        json!({"filename":"src/invitations.ts","status":"modified","additions":48,"deletions":9,"patch":"@@ -1,8 +1,13 @@\n import { randomBytes, createHash } from 'node:crypto';\n import { db } from './database';\n \n export async function inviteTeammate(email: string, organizationId: string) {\n-  const token = randomBytes(16).toString('hex');\n+  const token = randomBytes(32).toString('hex');\n+  const tokenHash = createHash('sha256').update(token).digest('hex');\n+  const normalizedEmail = email.trim().toLowerCase();\n   return db.invitation.create({\n-    data: { email, token },\n+    data: {\n+      email: normalizedEmail, organizationId, tokenHash,\n+      expiresAt: new Date(Date.now() + 86_400_000),\n+    },\n   });\n }"}),
        json!({"filename":"src/components/InviteDialog.tsx","status":"added","additions":92,"deletions":0,"patch":"@@ -0,0 +1,4 @@\n+export function InviteDialog() {\n+  return <Dialog aria-label=\"Invite a teammate\">\n+    <InviteForm />\n+  </Dialog>;"}),
        json!({"filename":"tests/invitations.test.ts","status":"modified","additions":46,"deletions":15,"patch":"@@ -1,2 +1,4 @@\n describe('invitations', () => {\n+  it('rejects expired invitations', async () => {\n+    await expect(acceptInvitation(expired)).rejects.toThrow();\n+  });\n });"}),
    ];
    v.selected_file = Some("src/invitations.ts".into());
    v.threads = vec![
        json!({"id":"PRRT_demo","path":"src/invitations.ts","line":10,"isResolved":false,"isOutdated":false,"comments":{"nodes":[{"id":"PRRC_demo","databaseId":100,"author":{"login":"sam"},"createdAt":"2026-10-25T14:42:00Z","body":"Could we also verify that the invitation belongs to the active organization before accepting it?"}]}}),
    ];
    if scene == 4 || scene == 31 || (36..=39).contains(&scene) {
        v.form = Some(Form::Create);
        v.base = "main".into();
        v.form_default = "main".into();
        v.form_current = "feat/team-invites".into();
        v.branches = vec!["main".into(), "develop".into(), "release/next".into(), "feat/team-invites".into(), "fix/session-timeout".into()];
        v.local_branches = vec![
            LocalBranch { name: "feat/team-invites".into(), upstream: "origin/feat/team-invites".into(), unpushed: 2 },
            LocalBranch { name: "feat/audit-log".into(), upstream: String::new(), unpushed: 0 },
            LocalBranch { name: "main".into(), upstream: "origin/main".into(), unpushed: 0 },
        ];
        if scene == 36 {
            v.title.clear();
        } else {
            v.form_head = "feat/team-invites".into();
            v.title = "Add secure team invitations".into();
            v.comparison = Some(json!({"known":true,"local":true,"pushed":true,"ahead":4,"behind":1,"files":7,"additions":186,"deletions":24,"unpushed":2,"mergeable":true,"subject":""}));
        }
        v.templates = templates();
        v.template = Some(0);
        if scene != 36 {
            // As "Describe with AI" leaves it: the template filled in, and
            // Undo back to the bare template.
            v.description = "## Summary\nInvite teammates by email with expiring, single-use links.\n\n## Changes\n- Hash invitation tokens and store only the hash\n- Expire invitations after 24 hours\n- Accessible error feedback in the invite dialog\n\n## Testing\n- [x] Unit tests for expiry and organization scoping\n- [ ] Manual check of the email flow".into();
            v.body_undo = Some(v.templates[0].body.clone());
        } else {
            v.description = v.templates[0].body.clone();
        }
        v.body_auto = Some(v.description.clone());
        v.picker = match scene {
            37 => Some(Picker::Source),
            38 => Some(Picker::Template),
            _ => None,
        };
        v.description_preview = scene == 39;
        ctx.update_view(&v.title_editor, |e, c| e.set_text(&v.title, c));
        ctx.update_view(&v.description_editor, |e, c| e.set_text(&v.description, c));
    }
    if scene == 6 {
        v.items.clear();
        v.detail = None;
        v.selected = None;
    }
    if scene == 8 {
        v.confirmation = Some(Operation::Merge {
            number: 128,
            head: "a".repeat(40),
            method: MergeMethod::Squash,
            auto: false,
        });
    }
    if scene == 9 {
        v.split = true;
    }
    v.check_log = None;
    if scene == 40 {
        // A failed check's log, long enough to need the viewer.
        let mut raw = String::new();
        for n in 0..1_800 {
            raw.push_str(&format!("build\tRun tests\t2026-10-31T12:00:{:02}.0000000Z test suite::case_{n} ... ok\n", n % 60));
        }
        raw.push_str("build\tRun tests\t2026-10-31T12:01:00.0000000Z ##[error]test invitations::accept_from_another_org ... FAILED\n");
        raw.push_str("build\tRun tests\t2026-10-31T12:01:00.1000000Z thread 'invitations::accept_from_another_org' panicked at src/invitations.rs:42:9\n");
        raw.push_str("build\tRun tests\t2026-10-31T12:01:00.2000000Z ##[error]Process completed with exit code 101.\n");
        let mut log = super::check_log::CheckLog::loading(18_234_771_022);
        log.fill(raw);
        v.check_log = Some(log);
    }
    if scene == 10 {
        v.pending = vec![LineComment {
            path: "src/invitations.ts".into(),
            line: 10,
            side: "RIGHT".into(),
            body:
                "Verify that the invitation belongs to the active organization before accepting it."
                    .into(),
            start_line: Some(7),
            start_side: Some("RIGHT".into()),
        }];
        v.form = Some(Form::ReviewDrafts);
    }
    if scene == 11 {
        v.confirmation = Some(Operation::OverrideMerge {
            number: 128,
            head: "a".repeat(40),
            method: MergeMethod::Squash,
            acknowledgement: String::new(),
        });
    }
    if scene == 13 {
        if let Some(item) = v.detail.as_mut() {
            item.data["mergeStateStatus"] = json!("CLEAN");
            item.data["reviewDecision"] = json!("APPROVED");
            item.data["statusCheckRollup"] =
                json!([{"name":"All checks","status":"COMPLETED","conclusion":"SUCCESS"}]);
        }
    }
    if (14..=18).contains(&scene) {
        for item in &mut v.items {
            item.data["provider"] = json!("gitlab");
            item.data["url"] = json!(format!(
                "https://gitlab.example/acme/app/-/merge_requests/{}",
                item.number()
            ));
            item.data["statusCheckRollup"] = json!([
                {"name":"Build","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://gitlab.example/acme/app/-/jobs/101","runId":101},
                {"name":"Integration tests","status":"COMPLETED","conclusion":"FAILURE","detailsUrl":"https://gitlab.example/acme/app/-/jobs/102","runId":102},
                {"name":"Accessibility","status":"IN_PROGRESS","conclusion":"","detailsUrl":"https://gitlab.example/acme/app/-/jobs/103","runId":103}
            ]);
        }
        v.detail = v.items.first().cloned();
        v.repository["provider"] = json!("gitlab");
        v.repository["rebaseMergeAllowed"] = json!(false);
        v.review = ReviewKind::Comment;
        v.threads[0]["id"] = json!("128:demo-thread");
        if scene == 16 {
            v.form = Some(Form::Create);
            v.form_is_gitlab = true;
            v.title = "Add secure team invitations".into();
            v.description = "## Summary\nSecure, expiring invitations with accessible feedback.".into();
            v.base = "main".into();
            v.form_default = "main".into();
            v.branches = vec!["main".into(), "develop".into()];
            v.form_current = "feat/team-invites".into();
            v.form_head = "feat/team-invites".into();
            v.local_branches = vec![LocalBranch { name: "feat/team-invites".into(), upstream: String::new(), unpushed: 0 }];
            v.comparison = Some(json!({"known":true,"local":true,"pushed":false,"ahead":2,"behind":0,"files":3,"additions":92,"deletions":4,"unpushed":2,"mergeable":false,"subject":""}));
            ctx.update_view(&v.title_editor, |e, c| e.set_text(&v.title, c));
            ctx.update_view(&v.description_editor, |e, c| e.set_text(&v.description, c));
        }
    }
    if scene == 18 {
        v.form = Some(Form::Connections);
    }
    if scene == 19 || scene == 20 {
        v.items.clear();
        v.detail = None;
        v.selected = None;
        v.loading = scene == 19;
        if scene == 20 {
            v.errors.push("Acme: Connection timed out".into());
        }
    }
    if scene == 21 {
        v.loading = true;
        v.detail_loading = true;
        v.errors
            .push("Acme: Last refresh timed out. Showing the previous results.".into());
    }
    if (25..=27).contains(&scene) {
        v.detail_loading = true;
        v.files.clear();
        v.threads.clear();
        if let Some(detail) = v.detail.as_mut() {
            for key in ["comments", "reviews", "commits"] {
                detail.data.as_object_mut().unwrap().remove(key);
            }
            if scene == 25 {
                detail.data.as_object_mut().unwrap().remove("body");
            }
        }
        v.tab = match scene {
            26 => Tab::Files,
            27 => Tab::Activity,
            _ => Tab::Overview,
        };
    }
    if (28..=33).contains(&scene) {
        v.picker = Some(match scene {
            29 => Picker::State,
            30 => Picker::Scope,
            31 => Picker::Base,
            32 => Picker::Merge,
            _ => Picker::Project,
        });
        if scene == 28 || scene == 33 {
            v.targets.extend([
                Target {
                    root: "/demo/api".into(),
                    label: "API platform".into(),
                },
                Target {
                    root: "/demo/mobile".into(),
                    label: "Mobile app".into(),
                },
            ]);
        }
        if scene == 31 {
            v.branches = (0..20).map(|i| format!("feature/review-{i}")).collect();
            v.branches.insert(0, "main".into());
        }
        if scene == 32 {
            if let Some(detail) = v.detail.as_mut() {
                detail.data["body"] =
                    json!("Improves invitation validation and the team onboarding flow.");
                detail.data["mergeStateStatus"] = json!("CLEAN");
                detail.data["reviewDecision"] = json!("APPROVED");
                detail.data["statusCheckRollup"] = json!([]);
            }
            v.threads.clear();
        }
    }
    v.sync_placeholders(ctx);
    ctx.notify();
    if scene == 34 {
        v.tab = Tab::Activity;
        v.threads.clear();
        if let Some(detail) = v.detail.as_mut() {
            detail.data["state"] = json!("MERGED");
            detail.data["comments"] = json!([]);
            detail.data["reviews"] = json!([]);
        }
    }
    if scene == 35 {
        v.tab = Tab::Files;
        if let Some(detail) = v.detail.as_mut() {
            detail.data["state"] = json!("MERGED");
        }
        v.line = Some(LineComment {
            path: "src/invitations.ts".into(),
            line: 7,
            side: "RIGHT".into(),
            body: String::new(),
            start_line: None,
            start_side: None,
        });
        v.body = "Could we add coverage for expired invitations?".into();
        ctx.update_view(&v.composer, |e, c| e.set_text(&v.body, c));
    }
}

/// Exercises real view action handlers against sample state without any remote writes.
pub(crate) fn smoke(v: &mut PullRequestsView, ctx: &mut ViewContext<PullRequestsView>) {
    use warpui::TypedActionView;
    v.handle_action(
        &Action::Line {
            path: "src/invitations.ts".into(),
            line: 7,
            side: "RIGHT".into(),
        },
        ctx,
    );
    v.handle_action(&Action::ExtendLine, ctx);
    v.handle_action(
        &Action::Line {
            path: "src/invitations.ts".into(),
            line: 10,
            side: "RIGHT".into(),
        },
        ctx,
    );
    assert_eq!(v.line.as_ref().unwrap().start_line, Some(7));
    assert_eq!(v.line.as_ref().unwrap().line, 10);
    v.body = "Please verify organization isolation here".into();
    v.handle_action(&Action::AddLine, ctx);
    assert_eq!(v.pending.len(), 1);
    v.handle_action(&Action::ShowDrafts, ctx);
    assert_eq!(v.form, Some(Form::ReviewDrafts));
    v.handle_action(&Action::Cancel, ctx);
    assert!(v.form.is_none());
    v.handle_action(&Action::Split, ctx);
    assert!(v.split);
    v.handle_action(&Action::Viewed("src/invitations.ts".into()), ctx);
    assert!(!v.viewed.is_empty());
    v.handle_action(&Action::New, ctx);
    assert_eq!(v.form, Some(Form::Create));
    assert!(v.branches.contains(&"main".into()));
    // Nothing is chosen for the person: the checked-out branch is offered.
    assert!(v.form_head.is_empty());
    assert_eq!(v.form_current, "feat/team-invites");
    assert!(!v.can_create());
    v.handle_action(&Action::Source("develop".into()), ctx);
    assert_eq!(v.form_head, "develop");
    assert_eq!(v.title, "Develop", "an empty title is suggested from the branch");
    v.handle_action(&Action::Source("feat/team-invites".into()), ctx);
    assert_eq!(v.title, "Team invites", "a suggested title follows the branch");
    v.handle_action(&Action::Base("develop".into()), ctx);
    assert!(v.can_create());
    v.handle_action(&Action::Base("feat/team-invites".into()), ctx);
    assert!(!v.can_create(), "base and source must differ");
    // A new description starts as the team's template.
    assert_eq!(v.template, Some(0));
    assert_eq!(v.description, v.templates[0].body);
    v.handle_action(&Action::Template(Some(1)), ctx);
    assert_eq!(v.description, v.templates[1].body);
    assert!(!v.can_undo_description(), "swapping one template for another needs no undo");
    v.description = "My own notes".into();
    v.handle_action(&Action::Template(Some(0)), ctx);
    assert_eq!(v.description, v.templates[0].body);
    assert!(v.can_undo_description(), "what the person wrote can come back");
    v.handle_action(&Action::UndoDescription, ctx);
    assert_eq!(v.description, "My own notes");
    v.handle_action(&Action::DescriptionTab(true), ctx);
    assert!(v.description_preview);
    // The inbox refreshing re-selects reviews and restores their comment
    // drafts: none of that reaches the description, and the description
    // never becomes a review's comment draft.
    let comment = v.body.clone();
    let first = v.items.first().map(Item::key);
    v.select_item(None, ctx);
    v.select_item(first.clone(), ctx);
    assert_eq!(v.description, "My own notes", "a refresh must not touch the description");
    assert_eq!(v.title, "Team invites");
    assert!(v.review_drafts.values().all(|d| d.body != "My own notes"));
    v.body = comment;
    // Leaving the page keeps the dialog as it was.
    v.hide(ctx);
    assert_eq!(v.form, Some(Form::Create));
    assert_eq!(v.description, "My own notes");
    v.visible = true;
    // Closing it keeps the unsent pull request for next time.
    v.handle_action(&Action::Cancel, ctx);
    assert!(v.form.is_none());
    v.handle_action(&Action::New, ctx);
    assert_eq!(v.description, "My own notes");
    assert_eq!(v.title, "Team invites");
    v.handle_action(&Action::Cancel, ctx);
    v.handle_action(
        &Action::Ask(Operation::Merge {
            number: 128,
            head: "a".repeat(40),
            method: MergeMethod::Squash,
            auto: false,
        }),
        ctx,
    );
    assert!(v.confirmation.is_some());
    v.handle_action(&Action::Cancel, ctx);
    assert!(v.confirmation.is_none());
    v.handle_action(&Action::BackToInbox, ctx);
    assert!(v.list_only);
    v.handle_action(&Action::TogglePicker(Picker::Project), ctx);
    assert_eq!(v.picker, Some(Picker::Project));
    v.handle_action(&Action::TogglePicker(Picker::Scope), ctx);
    assert_eq!(v.picker, Some(Picker::Scope));
    v.handle_action(&Action::ClosePicker, ctx);
    assert!(v.picker.is_none());
    v.handle_action(&Action::New, ctx);
    v.handle_action(&Action::TogglePicker(Picker::Base), ctx);
    v.handle_action(&Action::Escape, ctx);
    assert!(v.picker.is_none());
    assert_eq!(v.form, Some(Form::Create));
    v.handle_action(&Action::Cancel, ctx);
    if let Some(detail) = v.detail.as_mut() {
        detail.data["state"] = json!("MERGED");
    }
    v.handle_action(
        &Action::Line {
            path: "src/invitations.ts".into(),
            line: 7,
            side: "RIGHT".into(),
        },
        ctx,
    );
    assert_eq!(v.line.as_ref().unwrap().line, 7);
    assert_eq!(v.review, ReviewKind::Comment);
    v.body = "Follow-up on this merged change".into();
    v.handle_action(&Action::AddLine, ctx);
    assert!(v.can_submit_review());
    v.review = ReviewKind::Approve;
    assert!(!v.can_submit_review());
    let selected=v.selected.clone();
    v.handoff("Review the code and identify actionable findings",ctx);
    assert!(!v.busy,"Opening review setup must not start fetching");
    assert_eq!(v.selected,selected,"Setup keeps the selected review open");
    v.handoff("open",ctx);
    assert!(!v.busy,"Open workspace must wait for confirmation before fetching");
    println!("Pull request native interaction smoke checks passed (no remote writes)");
    seed(v, 0, ctx);
}
