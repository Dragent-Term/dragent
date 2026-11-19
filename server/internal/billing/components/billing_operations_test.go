package components

import (
	"context"
	"database/sql"
	"testing"

	"github.com/google/uuid"
	"server/internal/billing/model"
)

type fakeOperationBalanceRepo struct {
	*fakeBalRepo
	ops map[uuid.UUID]*model.BillingOperation
}

func newFakeOperationBalanceRepo(bal *model.UserBalance) *fakeOperationBalanceRepo {
	return &fakeOperationBalanceRepo{fakeBalRepo: &fakeBalRepo{bal: bal}, ops: make(map[uuid.UUID]*model.BillingOperation)}
}
func (f *fakeOperationBalanceRepo) GetOperationByKeyTx(_ context.Context, _ any, key string) (*model.BillingOperation, error) {
	for _, op := range f.ops {
		if op.OperationKey == key {
			copy := *op
			return &copy, nil
		}
	}
	return nil, sql.ErrNoRows
}
func (f *fakeOperationBalanceRepo) GetOperationByIDTx(_ context.Context, _ any, id uuid.UUID) (*model.BillingOperation, error) {
	op, ok := f.ops[id]
	if !ok {
		return nil, sql.ErrNoRows
	}
	copy := *op
	return &copy, nil
}
func (f *fakeOperationBalanceRepo) CreateOperationTx(_ context.Context, _ any, op *model.BillingOperation) error {
	copy := *op
	f.ops[op.ID] = &copy
	return nil
}
func (f *fakeOperationBalanceRepo) ActiveHoldsByPoolTx(_ context.Context, _ any, userID uuid.UUID) (map[string]int64, error) {
	holds := make(map[string]int64)
	for _, op := range f.ops {
		if op.UserID == userID && (op.Status == model.OperationAuthorized || op.Status == model.OperationUsageRecorded || op.Status == model.OperationSettlementFailed) {
			holds[op.Pool] += op.MinimumHoldMc
		}
	}
	return holds, nil
}
func (f *fakeOperationBalanceRepo) RecordOperationUsage(_ context.Context, id uuid.UUID, in, cached, out, flat int64, callType string) error {
	op := f.ops[id]
	op.InputTokens, op.CachedInputTokens, op.OutputTokens, op.FlatFeeMc = in, cached, out, flat
	op.CallType, op.Status = callType, model.OperationUsageRecorded
	return nil
}
func (f *fakeOperationBalanceRepo) MarkOperationSettledTx(_ context.Context, _ any, id, txID uuid.UUID, api, charged int64, pool string) error {
	op := f.ops[id]
	op.Status, op.ApiCostMc, op.UserChargedMc, op.Pool = model.OperationSettled, api, charged, pool
	op.CreditTransactionID = &txID
	return nil
}
func (f *fakeOperationBalanceRepo) MarkOperationSettlementFailed(_ context.Context, id uuid.UUID, message string) error {
	f.ops[id].Status, f.ops[id].LastError = model.OperationSettlementFailed, message
	return nil
}
func (f *fakeOperationBalanceRepo) VoidOperation(_ context.Context, id uuid.UUID, reason string) error {
	f.ops[id].Status, f.ops[id].LastError = model.OperationVoided, reason
	return nil
}
func (f *fakeOperationBalanceRepo) ListUnsettledOperations(_ context.Context, _ int) ([]*model.BillingOperation, error) {
	var out []*model.BillingOperation
	for _, op := range f.ops {
		if op.Status == model.OperationUsageRecorded || op.Status == model.OperationSettlementFailed {
			copy := *op
			out = append(out, &copy)
		}
	}
	return out, nil
}

func newOperationUC(bal *model.UserBalance) (*BillingUseCase, *fakeOperationBalanceRepo, *fakeTxRepo) {
	repo := newFakeOperationBalanceRepo(bal)
	txRepo := &fakeTxRepo{}
	return NewBillingUseCase(repo, &fakePriceRepo{price: grokBuildPrice()}, txRepo,
		&fakeProfitRepo{}, fakeConfig{}, fakeTxMgr{}), repo, txRepo
}

func TestAuthorizeOperationIsIdempotentAndAccountsForActiveHolds(t *testing.T) {
	uid := uuid.New()
	_, reset := freshWindows()
	uc, _, _ := newOperationUC(&model.UserBalance{UserID: uid, Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc: 7_500, FreeMonthlyResetAt: reset})
	cmd := AuthorizeOperationCommand{OperationKey: "task:1", UserID: uid, ModelName: "grok-build-0.1"}
	first, err := uc.AuthorizeOperation(context.Background(), cmd)
	if err != nil {
		t.Fatal(err)
	}
	again, err := uc.AuthorizeOperation(context.Background(), cmd)
	if err != nil {
		t.Fatal(err)
	}
	if first.ID != again.ID {
		t.Fatalf("idempotent authorization changed ID: %s != %s", first.ID, again.ID)
	}
	cmd.OperationKey = "task:2"
	if _, err := uc.AuthorizeOperation(context.Background(), cmd); err == nil {
		t.Fatal("second concurrent call must not reuse capacity held by the first")
	}
}

func TestSettleOperationChargesExactlyOnce(t *testing.T) {
	uid := uuid.New()
	_, reset := freshWindows()
	uc, repo, txRepo := newOperationUC(&model.UserBalance{UserID: uid, Tier: model.TierPro, IsPaid: true,
		PaidBalanceMc: 100_000, FreeMonthlyResetAt: reset})
	op, err := uc.AuthorizeOperation(context.Background(), AuthorizeOperationCommand{
		OperationKey: "task:round:1", UserID: uid, ModelName: "grok-build-0.1"})
	if err != nil {
		t.Fatal(err)
	}
	cmd := ChargeUserCommand{UserID: uid, ModelName: "grok-build-0.1", InputTokens: 100, OutputTokens: 50}
	first, err := uc.SettleOperation(context.Background(), op.ID, cmd)
	if err != nil {
		t.Fatal(err)
	}
	second, err := uc.SettleOperation(context.Background(), op.ID, cmd)
	if err != nil {
		t.Fatal(err)
	}
	if len(txRepo.txs) != 1 {
		t.Fatalf("ledger rows=%d, want 1", len(txRepo.txs))
	}
	if first.TransactionID != second.TransactionID {
		t.Fatal("idempotent settlement returned a different transaction")
	}
	if repo.ops[op.ID].Status != model.OperationSettled {
		t.Fatalf("status=%s", repo.ops[op.ID].Status)
	}
}
