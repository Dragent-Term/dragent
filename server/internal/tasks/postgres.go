package tasks

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgconn"
	"github.com/jackc/pgx/v5/pgxpool"

	"server/internal/organization"
)

// PostgresRepository stores tasks in Postgres. Every write the handler
// makes sets its times explicitly (truncated to milliseconds), so page
// cursors and incremental refreshes compare exact values.
type PostgresRepository struct {
	pool *pgxpool.Pool
}

func NewPostgresRepository(pool *pgxpool.Pool) *PostgresRepository {
	return &PostgresRepository{pool: pool}
}

const displayName = `COALESCE(NULLIF(TRIM(%s.display_name), ''), NULLIF(TRIM(%s.first_name || ' ' || %s.last_name), ''), %s.email)`

const taskColumns = `
	t.id, t.organization_id, t.number, t.key, t.title, t.description, t.status, t.priority,
	t.assignee_user_id, au.email, COALESCE(NULLIF(TRIM(au.display_name), ''), NULLIF(TRIM(au.first_name || ' ' || au.last_name), ''), au.email),
	t.creator_user_id, cu.email, COALESCE(NULLIF(TRIM(cu.display_name), ''), NULLIF(TRIM(cu.first_name || ' ' || cu.last_name), ''), cu.email),
	t.project_key, t.project_label, t.labels, t.due_at, t.created_at, t.updated_at,
	t.started_at, t.completed_at, t.deleted_at,
	(SELECT COUNT(*) FROM task_comments c WHERE c.task_id = t.id)`

const taskJoins = `
	FROM tasks t
	LEFT JOIN users au ON au.id = t.assignee_user_id
	LEFT JOIN users cu ON cu.id = t.creator_user_id`

func scanTask(row pgx.Row) (Task, error) {
	var t Task
	var assigneeID, creatorID *uuid.UUID
	var assigneeEmail, assigneeName, creatorEmail, creatorName *string
	var projectKey, projectLabel *string
	var dueAt, createdAt, updatedAt, startedAt, completedAt, deletedAt *time.Time
	err := row.Scan(&t.ID, &t.OrgID, &t.Number, &t.Key, &t.Title, &t.Description,
		&t.Status, &t.Priority, &assigneeID, &assigneeEmail, &assigneeName,
		&creatorID, &creatorEmail, &creatorName, &projectKey, &projectLabel,
		&t.Labels, &dueAt, &createdAt, &updatedAt, &startedAt, &completedAt, &deletedAt,
		&t.CommentCount)
	if err != nil {
		return t, err
	}
	if assigneeID != nil {
		t.AssigneeID = assigneeID
		t.Assignee = &Person{ID: assigneeID.String(), Name: str(assigneeName), Email: str(assigneeEmail)}
	}
	if creatorID != nil {
		t.CreatorID = creatorID
		t.Creator = &Person{ID: creatorID.String(), Name: str(creatorName), Email: str(creatorEmail)}
	}
	if projectKey != nil {
		t.Project = &ProjectRef{Key: *projectKey, Label: str(projectLabel)}
	}
	if t.Labels == nil {
		t.Labels = []string{}
	}
	t.DueAtMs = millisOf(dueAt)
	t.CreatedAtMs = createdAt.UnixMilli()
	t.UpdatedAtMs = updatedAt.UnixMilli()
	t.StartedAtMs = millisOf(startedAt)
	t.CompletedAtMs = millisOf(completedAt)
	t.DeletedAtMs = millisOf(deletedAt)
	return t, nil
}

func str(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

func millisOf(t *time.Time) *int64 {
	if t == nil {
		return nil
	}
	ms := t.UnixMilli()
	return &ms
}

func (r *PostgresRepository) Member(ctx context.Context, orgID, userID uuid.UUID) (*Member, error) {
	var m Member
	m.UserID = userID
	var role string
	var name, email string
	err := r.pool.QueryRow(ctx, `
		SELECT m.role, u.email, `+fmt.Sprintf(displayName, "u", "u", "u", "u")+`
		FROM memberships m JOIN users u ON u.id = m.user_id
		WHERE m.organization_id = $1 AND m.user_id = $2`, orgID, userID).Scan(&role, &email, &name)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	m.Role = roleOf(role)
	m.Person = Person{ID: userID.String(), Name: name, Email: email}
	return &m, nil
}

func roleOf(role string) organization.MembershipRole {
	switch organization.MembershipRole(role) {
	case organization.RoleOwner, organization.RoleAdmin, organization.RoleMember:
		return organization.MembershipRole(role)
	default:
		// An unknown stored role never satisfies a gate; keep it visible.
		return organization.MembershipRole(role)
	}
}
func (r *PostgresRepository) EnsurePrefix(ctx context.Context, orgID uuid.UUID) (string, error) {
	var prefix string
	err := r.pool.QueryRow(ctx, `SELECT prefix FROM task_counters WHERE organization_id = $1`, orgID).Scan(&prefix)
	if err == nil {
		return prefix, nil
	}
	if !errors.Is(err, pgx.ErrNoRows) {
		return "", err
	}
	var name string
	if err := r.pool.QueryRow(ctx, `SELECT name FROM organizations WHERE id = $1`, orgID).Scan(&name); err != nil {
		return "", err
	}
	prefix = DerivePrefix(name)
	// Never changes once made: a concurrent maker wins and this keeps it.
	if _, err := r.pool.Exec(ctx, `INSERT INTO task_counters (organization_id, prefix, next_number)
		VALUES ($1, $2, 1) ON CONFLICT (organization_id) DO NOTHING`, orgID, prefix); err != nil {
		return "", err
	}
	if err := r.pool.QueryRow(ctx, `SELECT prefix FROM task_counters WHERE organization_id = $1`, orgID).Scan(&prefix); err != nil {
		return "", err
	}
	return prefix, nil
}

func (r *PostgresRepository) CreateTask(ctx context.Context, t Task, created Activity) (Task, error) {
	prefix, err := r.EnsurePrefix(ctx, t.OrgID)
	if err != nil {
		return Task{}, err
	}
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return Task{}, err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	var next int64
	if err := tx.QueryRow(ctx, `SELECT next_number FROM task_counters WHERE organization_id = $1 FOR UPDATE`, t.OrgID).Scan(&next); err != nil {
		return Task{}, err
	}
	t.Number = next
	t.Key = fmt.Sprintf("%s-%d", prefix, next)
	if _, err := tx.Exec(ctx, `
		INSERT INTO tasks (id, organization_id, number, key, title, description, status, priority,
			assignee_user_id, creator_user_id, project_key, project_label, labels, due_at,
			created_at, updated_at, started_at, completed_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)`,
		t.ID, t.OrgID, t.Number, t.Key, t.Title, t.Description, t.Status, t.Priority,
		t.AssigneeID, t.CreatorID, projectKeyOf(t.Project), projectLabelOf(t.Project), t.Labels,
		timeOf(t.DueAtMs), millisToTime(t.CreatedAtMs), millisToTime(t.UpdatedAtMs),
		timeOf(t.StartedAtMs), timeOf(t.CompletedAtMs)); err != nil {
		return Task{}, err
	}
	if err := insertActivity(ctx, tx, created); err != nil {
		return Task{}, err
	}
	if _, err := tx.Exec(ctx, `UPDATE task_counters SET next_number = $2 WHERE organization_id = $1`, t.OrgID, next+1); err != nil {
		return Task{}, err
	}
	if err := tx.Commit(ctx); err != nil {
		return Task{}, err
	}
	return t, nil
}

func projectKeyOf(p *ProjectRef) *string {
	if p == nil {
		return nil
	}
	return &p.Key
}

func projectLabelOf(p *ProjectRef) *string {
	if p == nil {
		return nil
	}
	return &p.Label
}

func timeOf(ms *int64) *time.Time {
	if ms == nil {
		return nil
	}
	t := millisToTime(*ms)
	return &t
}

func taskWhere(idOrKey string, n int) (string, any) {
	if id, err := uuid.Parse(idOrKey); err == nil {
		return fmt.Sprintf("t.id = $%d", n), id
	}
	return fmt.Sprintf("UPPER(t.key) = UPPER($%d)", n), idOrKey
}

func (r *PostgresRepository) GetTask(ctx context.Context, orgID uuid.UUID, idOrKey string) (*Task, error) {
	return r.getTask(ctx, orgID, idOrKey, false)
}

func (r *PostgresRepository) GetTaskRaw(ctx context.Context, orgID uuid.UUID, idOrKey string) (*Task, error) {
	return r.getTask(ctx, orgID, idOrKey, true)
}

func (r *PostgresRepository) getTask(ctx context.Context, orgID uuid.UUID, idOrKey string, includeDeleted bool) (*Task, error) {
	cond, arg := taskWhere(idOrKey, 2)
	sql := `SELECT ` + taskColumns + taskJoins + `
		WHERE t.organization_id = $1 AND ` + cond
	if !includeDeleted {
		sql += ` AND t.deleted_at IS NULL`
	}
	t, err := scanTask(r.pool.QueryRow(ctx, sql, orgID, arg))
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	m, err := r.linksMap(ctx, []uuid.UUID{t.ID})
	if err != nil {
		return nil, err
	}
	t.Links = m[t.ID]
	return &t, nil
}

func (r *PostgresRepository) UpdateTask(ctx context.Context, t Task, activities []Activity) error {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	tag, err := tx.Exec(ctx, `
		UPDATE tasks SET title = $2, description = $3, status = $4, priority = $5,
			assignee_user_id = $6, project_key = $7, project_label = $8, labels = $9,
			due_at = $10, updated_at = $11, started_at = $12, completed_at = $13
		WHERE id = $1 AND organization_id = $14 AND deleted_at IS NULL`,
		t.ID, t.Title, t.Description, t.Status, t.Priority, t.AssigneeID,
		projectKeyOf(t.Project), projectLabelOf(t.Project), t.Labels, timeOf(t.DueAtMs),
		millisToTime(t.UpdatedAtMs), timeOf(t.StartedAtMs), timeOf(t.CompletedAtMs), t.OrgID)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	for _, a := range activities {
		if err := insertActivity(ctx, tx, a); err != nil {
			return err
		}
	}
	return tx.Commit(ctx)
}

func (r *PostgresRepository) SoftDelete(ctx context.Context, orgID, taskID uuid.UUID, atMs int64, activity Activity) error {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	tag, err := tx.Exec(ctx, `UPDATE tasks SET deleted_at = $2, updated_at = $2
		WHERE id = $1 AND organization_id = $3 AND deleted_at IS NULL`,
		taskID, millisToTime(atMs), orgID)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	if err := insertActivity(ctx, tx, activity); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

func (r *PostgresRepository) Restore(ctx context.Context, orgID, taskID uuid.UUID, atMs int64, activity Activity) error {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	tag, err := tx.Exec(ctx, `UPDATE tasks SET deleted_at = NULL, updated_at = $2
		WHERE id = $1 AND organization_id = $3 AND deleted_at IS NOT NULL`,
		taskID, millisToTime(atMs), orgID)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	if err := insertActivity(ctx, tx, activity); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

func (r *PostgresRepository) ListTasks(ctx context.Context, orgID uuid.UUID, opts ListOptions) (ListResult, error) {
	var out ListResult
	out.DeletedIDs = []string{}
	args := []any{orgID}
	var conds []string
	conds = append(conds, "t.deleted_at IS NULL")
	if len(opts.Statuses) > 0 {
		args = append(args, opts.Statuses)
		conds = append(conds, fmt.Sprintf("t.status = ANY($%d)", len(args)))
	}
	if opts.AssigneeNone {
		conds = append(conds, "t.assignee_user_id IS NULL")
	} else if opts.AssigneeID != nil {
		args = append(args, *opts.AssigneeID)
		conds = append(conds, fmt.Sprintf("t.assignee_user_id = $%d", len(args)))
	}
	if opts.Label != "" {
		args = append(args, opts.Label)
		conds = append(conds, fmt.Sprintf("$%d = ANY(t.labels)", len(args)))
	}
	if opts.Project != "" {
		args = append(args, opts.Project)
		conds = append(conds, fmt.Sprintf("t.project_key = $%d", len(args)))
	}
	for _, w := range opts.Words {
		pat := "%" + EscapeLike(w) + "%"
		args = append(args, pat, pat, pat)
		conds = append(conds, fmt.Sprintf("(t.key ILIKE $%d ESCAPE '\\' OR t.title ILIKE $%d ESCAPE '\\' OR t.description ILIKE $%d ESCAPE '\\')",
			len(args)-2, len(args)-1, len(args)))
	}
	if opts.UpdatedAfterMs != nil {
		args = append(args, millisToTime(*opts.UpdatedAfterMs))
		conds = append(conds, fmt.Sprintf("t.updated_at > $%d", len(args)))
	}
	if opts.Cursor != nil {
		args = append(args, millisToTime(opts.Cursor.UpdatedMs), opts.Cursor.ID)
		// Compare ids as UUIDs, the same order the page is sorted in.
		conds = append(conds, fmt.Sprintf("(t.updated_at < $%d OR (t.updated_at = $%d AND t.id > $%d::uuid))",
			len(args)-1, len(args)-1, len(args)))
	}
	limit := opts.Limit
	if limit <= 0 {
		limit = 200
	}
	if limit > 500 {
		limit = 500
	}
	args = append(args, limit+1)
	sql := `SELECT ` + taskColumns + taskJoins + `
		WHERE t.organization_id = $1 AND ` + strings.Join(conds, " AND ") + `
		ORDER BY t.updated_at DESC, t.id ASC LIMIT $` + fmt.Sprintf("%d", len(args))
	rows, err := r.pool.Query(ctx, sql, args...)
	if err != nil {
		return out, err
	}
	defer rows.Close()
	var ids []uuid.UUID
	for rows.Next() {
		t, err := scanTask(rows)
		if err != nil {
			return out, err
		}
		out.Tasks = append(out.Tasks, t)
		ids = append(ids, t.ID)
	}
	if err := rows.Err(); err != nil {
		return out, err
	}
	if len(out.Tasks) > limit {
		out.Tasks = out.Tasks[:limit]
		ids = ids[:limit]
		last := out.Tasks[len(out.Tasks)-1]
		c := encodeCursor(pageCursor{UpdatedMs: last.UpdatedAtMs, ID: last.ID.String()})
		out.NextCursor = &c
	}
	if len(ids) > 0 {
		linkMap, err := r.linksMap(ctx, ids)
		if err != nil {
			return out, err
		}
		for i := range out.Tasks {
			out.Tasks[i].Links = linkMap[out.Tasks[i].ID]
		}
	}
	if out.Tasks == nil {
		out.Tasks = []Task{}
	}
	if opts.UpdatedAfterMs != nil {
		// Deletions since the client's timestamp, so it can drop them.
		delRows, err := r.pool.Query(ctx, `SELECT id FROM tasks
			WHERE organization_id = $1 AND deleted_at IS NOT NULL AND updated_at > $2
			ORDER BY updated_at DESC`, orgID, millisToTime(*opts.UpdatedAfterMs))
		if err != nil {
			return out, err
		}
		defer delRows.Close()
		for delRows.Next() {
			var id uuid.UUID
			if err := delRows.Scan(&id); err != nil {
				return out, err
			}
			out.DeletedIDs = append(out.DeletedIDs, id.String())
		}
		if err := delRows.Err(); err != nil {
			return out, err
		}
	}
	return out, nil
}

func (r *PostgresRepository) TaskLabels(ctx context.Context, orgID uuid.UUID) ([]string, error) {
	rows, err := r.pool.Query(ctx, `SELECT DISTINCT label FROM tasks, UNNEST(labels) AS label
		WHERE organization_id = $1 AND deleted_at IS NULL ORDER BY label`, orgID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	labels := []string{}
	for rows.Next() {
		var l string
		if err := rows.Scan(&l); err != nil {
			return nil, err
		}
		labels = append(labels, l)
	}
	return labels, rows.Err()
}

const commentColumns = `
	c.id, c.organization_id, c.task_id, c.author_user_id, u.email,
	COALESCE(NULLIF(TRIM(u.display_name), ''), NULLIF(TRIM(u.first_name || ' ' || u.last_name), ''), u.email),
	c.actor_kind, c.body, c.created_at, c.edited_at`

const commentJoins = `
	FROM task_comments c LEFT JOIN users u ON u.id = c.author_user_id`

func scanComment(row pgx.Row) (Comment, error) {
	var c Comment
	var authorID *uuid.UUID
	var email, name *string
	var createdAt time.Time
	var editedAt *time.Time
	err := row.Scan(&c.ID, &c.OrgID, &c.TaskID, &authorID, &email, &name,
		&c.ActorKind, &c.Body, &createdAt, &editedAt)
	if err != nil {
		return c, err
	}
	if authorID != nil {
		c.AuthorID = authorID
		c.Author = &Person{ID: authorID.String(), Name: str(name), Email: str(email)}
	}
	c.CreatedAtMs = createdAt.UnixMilli()
	c.EditedAtMs = millisOf(editedAt)
	return c, nil
}

func (r *PostgresRepository) TaskComments(ctx context.Context, orgID, taskID uuid.UUID) ([]Comment, error) {
	rows, err := r.pool.Query(ctx, `SELECT `+commentColumns+commentJoins+`
		WHERE c.organization_id = $1 AND c.task_id = $2
		ORDER BY c.created_at ASC, c.id ASC`, orgID, taskID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Comment{}
	for rows.Next() {
		c, err := scanComment(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, c)
	}
	return out, rows.Err()
}

func (r *PostgresRepository) CreateComment(ctx context.Context, c Comment) (Comment, error) {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return Comment{}, err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	if _, err := tx.Exec(ctx, `
		INSERT INTO task_comments (id, organization_id, task_id, author_user_id, actor_kind, body, created_at, updated_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $7)`,
		c.ID, c.OrgID, c.TaskID, c.AuthorID, c.ActorKind, c.Body, millisToTime(c.CreatedAtMs)); err != nil {
		return Comment{}, err
	}
	if err := touchTask(ctx, tx, c.OrgID, c.TaskID, c.CreatedAtMs); err != nil {
		return Comment{}, err
	}
	if err := tx.Commit(ctx); err != nil {
		return Comment{}, err
	}
	return c, nil
}

func (r *PostgresRepository) GetComment(ctx context.Context, orgID, taskID, commentID uuid.UUID) (*Comment, error) {
	c, err := scanComment(r.pool.QueryRow(ctx, `SELECT `+commentColumns+commentJoins+`
		WHERE c.organization_id = $1 AND c.task_id = $2 AND c.id = $3`, orgID, taskID, commentID))
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &c, nil
}

func (r *PostgresRepository) UpdateComment(ctx context.Context, orgID, taskID, commentID uuid.UUID, body string, editedAtMs, touchedAtMs int64) error {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	tag, err := tx.Exec(ctx, `UPDATE task_comments SET body = $1, edited_at = $2, updated_at = $2
		WHERE id = $3 AND organization_id = $4 AND task_id = $5`,
		body, millisToTime(editedAtMs), commentID, orgID, taskID)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	if err := touchTask(ctx, tx, orgID, taskID, touchedAtMs); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

func (r *PostgresRepository) DeleteComment(ctx context.Context, orgID, taskID, commentID uuid.UUID, touchedAtMs int64) error {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	tag, err := tx.Exec(ctx, `DELETE FROM task_comments WHERE id = $1 AND organization_id = $2 AND task_id = $3`,
		commentID, orgID, taskID)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	if err := touchTask(ctx, tx, orgID, taskID, touchedAtMs); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

const activityColumns = `
	a.id, a.organization_id, a.task_id, a.actor_user_id, u.email,
	COALESCE(NULLIF(TRIM(u.display_name), ''), NULLIF(TRIM(u.first_name || ' ' || u.last_name), ''), u.email),
	a.actor_kind, a.kind, a.from_json, a.to_json, a.created_at`

const activityJoins = `
	FROM task_activity a LEFT JOIN users u ON u.id = a.actor_user_id`

func scanActivity(row pgx.Row) (Activity, error) {
	var a Activity
	var actorID *uuid.UUID
	var email, name *string
	var fromRaw, toRaw []byte
	var at time.Time
	err := row.Scan(&a.ID, &a.OrgID, &a.TaskID, &actorID, &email, &name,
		&a.ActorKind, &a.Kind, &fromRaw, &toRaw, &at)
	if err != nil {
		return a, err
	}
	if actorID != nil {
		a.ActorID = actorID
		a.Actor = &Person{ID: actorID.String(), Name: str(name), Email: str(email)}
	}
	if fromRaw != nil {
		var v any
		if err := json.Unmarshal(fromRaw, &v); err != nil {
			return a, err
		}
		a.From = v
	}
	if toRaw != nil {
		var v any
		if err := json.Unmarshal(toRaw, &v); err != nil {
			return a, err
		}
		a.To = v
	}
	a.AtMs = at.UnixMilli()
	return a, nil
}

func (r *PostgresRepository) TaskActivity(ctx context.Context, orgID, taskID uuid.UUID) ([]Activity, error) {
	rows, err := r.pool.Query(ctx, `SELECT `+activityColumns+activityJoins+`
		WHERE a.organization_id = $1 AND a.task_id = $2
		ORDER BY a.created_at ASC, a.id ASC`, orgID, taskID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Activity{}
	for rows.Next() {
		a, err := scanActivity(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, a)
	}
	return out, rows.Err()
}

const linkColumns = `
	l.id, l.organization_id, l.task_id, l.kind, l.title, l.url, l.reference, l.state,
	l.added_by_user_id, u.email,
	COALESCE(NULLIF(TRIM(u.display_name), ''), NULLIF(TRIM(u.first_name || ' ' || u.last_name), ''), u.email),
	l.created_at`

const linkJoins = `
	FROM task_links l LEFT JOIN users u ON u.id = l.added_by_user_id`

func scanLink(row pgx.Row) (TaskLinkRow, error) {
	var l TaskLinkRow
	var addedByID *uuid.UUID
	var email, name *string
	var createdAt time.Time
	err := row.Scan(&l.ID, &l.OrgID, &l.TaskID, &l.Kind, &l.Title, &l.URL, &l.Reference,
		&l.State, &addedByID, &email, &name, &createdAt)
	if err != nil {
		return l, err
	}
	if addedByID != nil {
		l.AddedByID = addedByID
		l.AddedBy = &Person{ID: addedByID.String(), Name: str(name), Email: str(email)}
	}
	l.CreatedAtMs = createdAt.UnixMilli()
	return l, nil
}

func (r *PostgresRepository) linksMap(ctx context.Context, taskIDs []uuid.UUID) (map[uuid.UUID][]TaskLinkRow, error) {
	out := map[uuid.UUID][]TaskLinkRow{}
	if len(taskIDs) == 0 {
		return out, nil
	}
	rows, err := r.pool.Query(ctx, `SELECT `+linkColumns+linkJoins+`
		WHERE l.task_id = ANY($1) ORDER BY l.created_at ASC, l.id ASC`, taskIDs)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	for rows.Next() {
		l, err := scanLink(rows)
		if err != nil {
			return nil, err
		}
		out[l.TaskID] = append(out[l.TaskID], l)
	}
	return out, rows.Err()
}

func (r *PostgresRepository) GetLink(ctx context.Context, orgID, taskID, linkID uuid.UUID) (*TaskLinkRow, error) {
	l, err := scanLink(r.pool.QueryRow(ctx, `SELECT `+linkColumns+linkJoins+`
		WHERE l.id = $1 AND l.organization_id = $2 AND l.task_id = $3`, linkID, orgID, taskID))
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &l, nil
}

func (r *PostgresRepository) FindLink(ctx context.Context, orgID, taskID uuid.UUID, kind, reference string) (*TaskLinkRow, error) {
	l, err := scanLink(r.pool.QueryRow(ctx, `SELECT `+linkColumns+linkJoins+`
		WHERE l.organization_id = $1 AND l.task_id = $2 AND l.kind = $3 AND l.reference = $4`,
		orgID, taskID, kind, reference))
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &l, nil
}

func (r *PostgresRepository) CreateLink(ctx context.Context, link TaskLinkRow, activity Activity, touchedAtMs int64) (TaskLinkRow, error) {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return TaskLinkRow{}, err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	if _, err := tx.Exec(ctx, `
		INSERT INTO task_links (id, organization_id, task_id, kind, title, url, reference, state, added_by_user_id, created_at, updated_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)`,
		link.ID, link.OrgID, link.TaskID, link.Kind, link.Title, link.URL, link.Reference,
		link.State, link.AddedByID, millisToTime(link.CreatedAtMs)); err != nil {
		return TaskLinkRow{}, err
	}
	if err := insertActivity(ctx, tx, activity); err != nil {
		return TaskLinkRow{}, err
	}
	if err := touchTask(ctx, tx, link.OrgID, link.TaskID, touchedAtMs); err != nil {
		return TaskLinkRow{}, err
	}
	if err := tx.Commit(ctx); err != nil {
		return TaskLinkRow{}, err
	}
	return link, nil
}

func (r *PostgresRepository) UpdateLink(ctx context.Context, orgID, taskID, linkID uuid.UUID, title *string, url, state NullableString, touchedAtMs int64) (*TaskLinkRow, error) {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return nil, err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	set := []string{}
	args := []any{linkID, orgID, taskID}
	if title != nil {
		args = append(args, *title)
		set = append(set, fmt.Sprintf("title = $%d", len(args)))
	}
	if url.Set {
		args = append(args, url.Value)
		set = append(set, fmt.Sprintf("url = $%d", len(args)))
	}
	if state.Set {
		args = append(args, state.Value)
		set = append(set, fmt.Sprintf("state = $%d", len(args)))
	}
	if len(set) > 0 {
		args = append(args, millisToTime(touchedAtMs))
		set = append(set, fmt.Sprintf("updated_at = $%d", len(args)))
		if _, err := tx.Exec(ctx, `UPDATE task_links SET `+strings.Join(set, ", ")+`
			WHERE id = $1 AND organization_id = $2 AND task_id = $3`, args...); err != nil {
			return nil, err
		}
	}
	var updated TaskLinkRow
	updated, err = scanLink(tx.QueryRow(ctx, `SELECT `+linkColumns+linkJoins+`
		WHERE l.id = $1 AND l.organization_id = $2 AND l.task_id = $3`, linkID, orgID, taskID))
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, err
	}
	if len(set) > 0 {
		if err := touchTask(ctx, tx, orgID, taskID, touchedAtMs); err != nil {
			return nil, err
		}
	}
	if err := tx.Commit(ctx); err != nil {
		return nil, err
	}
	return &updated, nil
}

func (r *PostgresRepository) DeleteLink(ctx context.Context, orgID, taskID, linkID uuid.UUID, activity Activity, touchedAtMs int64) error {
	tx, err := r.pool.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback(ctx) }()
	tag, err := tx.Exec(ctx, `DELETE FROM task_links WHERE id = $1 AND organization_id = $2 AND task_id = $3`,
		linkID, orgID, taskID)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	if err := insertActivity(ctx, tx, activity); err != nil {
		return err
	}
	if err := touchTask(ctx, tx, orgID, taskID, touchedAtMs); err != nil {
		return err
	}
	return tx.Commit(ctx)
}

type execer interface {
	Exec(ctx context.Context, sql string, args ...any) (pgconn.CommandTag, error)
	QueryRow(ctx context.Context, sql string, args ...any) pgx.Row
}

func insertActivity(ctx context.Context, tx execer, a Activity) error {
	fromRaw, err := marshalJSONOrNil(a.From)
	if err != nil {
		return err
	}
	toRaw, err := marshalJSONOrNil(a.To)
	if err != nil {
		return err
	}
	_, err = tx.Exec(ctx, `
		INSERT INTO task_activity (id, organization_id, task_id, actor_user_id, actor_kind, kind, from_json, to_json, created_at)
		VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)`,
		a.ID, a.OrgID, a.TaskID, a.ActorID, a.ActorKind, a.Kind, fromRaw, toRaw, millisToTime(a.AtMs))
	return err
}

func marshalJSONOrNil(v any) ([]byte, error) {
	if v == nil {
		return nil, nil
	}
	return json.Marshal(v)
}

func touchTask(ctx context.Context, tx execer, orgID, taskID uuid.UUID, atMs int64) error {
	_, err := tx.Exec(ctx, `UPDATE tasks SET updated_at = $2
		WHERE id = $1 AND organization_id = $3 AND deleted_at IS NULL`,
		taskID, millisToTime(atMs), orgID)
	return err
}
