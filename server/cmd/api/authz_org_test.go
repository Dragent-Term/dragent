package main

import (
	"context"
	"database/sql"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"server/internal/organization"
)

type fakeMembershipRepo struct {
	role organization.MembershipRole
	err  error
}

func (f fakeMembershipRepo) MembershipRole(ctx context.Context, userID, orgID uuid.UUID) (organization.MembershipRole, error) {
	return f.role, f.err
}

func newOrgRequest(userID, orgID string) *http.Request {
	req := httptest.NewRequest(http.MethodGet, "/organizations/"+orgID+"/whatever", nil)

	rctx := chi.NewRouteContext()
	rctx.URLParams.Add("orgID", orgID)
	ctx := context.WithValue(req.Context(), chi.RouteCtxKey, rctx)
	if userID != "" {
		ctx = context.WithValue(ctx, "user_id", userID)
	}
	return req.WithContext(ctx)
}

func TestRequireOrgRoleRejectsNonMember(t *testing.T) {
	repo := fakeMembershipRepo{err: sql.ErrNoRows}
	handlerReached := false
	h := RequireOrgRole(repo, organization.RoleMember)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		handlerReached = true
	}))

	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, newOrgRequest(uuid.New().String(), uuid.New().String()))

	if handlerReached {
		t.Fatal("a non-member must never reach the handler")
	}
	if rec.Code != http.StatusForbidden {
		t.Fatalf("want 403 for a non-member, got %d", rec.Code)
	}
}

func TestRequireOrgRoleAllowsSufficientRole(t *testing.T) {
	repo := fakeMembershipRepo{role: organization.RoleAdmin}
	handlerReached := false
	h := RequireOrgRole(repo, organization.RoleAdmin)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		handlerReached = true
		w.WriteHeader(http.StatusOK)
	}))

	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, newOrgRequest(uuid.New().String(), uuid.New().String()))

	if !handlerReached {
		t.Fatal("a member with the required role must reach the handler")
	}
	if rec.Code != http.StatusOK {
		t.Fatalf("want 200, got %d", rec.Code)
	}
}

func TestRequireOrgRoleRejectsInsufficientRole(t *testing.T) {
	repo := fakeMembershipRepo{role: organization.RoleMember}
	handlerReached := false
	h := RequireOrgRole(repo, organization.RoleAdmin)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		handlerReached = true
	}))

	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, newOrgRequest(uuid.New().String(), uuid.New().String()))

	if handlerReached {
		t.Fatal("a plain member must not pass an admin-required gate")
	}
	if rec.Code != http.StatusForbidden {
		t.Fatalf("want 403 for insufficient role, got %d", rec.Code)
	}
}

func TestRequireOrgRoleRejectsMissingUser(t *testing.T) {
	repo := fakeMembershipRepo{role: organization.RoleOwner}
	handlerReached := false
	h := RequireOrgRole(repo, organization.RoleMember)(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		handlerReached = true
	}))

	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, newOrgRequest("", uuid.New().String()))

	if handlerReached {
		t.Fatal("a request with no authenticated user must not reach the handler")
	}
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("want 401 for a missing user, got %d", rec.Code)
	}
}
