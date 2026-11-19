package tasks

import (
	"testing"

	"github.com/google/uuid"
)

func TestGetForUserEnforcesTaskOwnership(t *testing.T) {
	manager := NewManager(nil, nil, nil, nil, nil)
	ownerID := uuid.New()
	otherUserID := uuid.New()
	task := &Task{ID: uuid.New(), UserID: ownerID}
	manager.tasks[task.ID] = task

	got, ok := manager.GetForUser(task.ID, ownerID)
	if !ok || got != task {
		t.Fatal("task owner should be able to retrieve the task")
	}

	if got, ok := manager.GetForUser(task.ID, otherUserID); ok || got != nil {
		t.Fatal("a different authenticated user must not retrieve the task")
	}

	if got, ok := manager.GetForUser(task.ID, uuid.Nil); ok || got != nil {
		t.Fatal("an unauthenticated caller must not retrieve the task")
	}

	if got, ok := manager.GetForUser(uuid.New(), ownerID); ok || got != nil {
		t.Fatal("an unknown task must not be retrievable")
	}
}
