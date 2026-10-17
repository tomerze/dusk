package campaign

import (
	"context"
	"errors"

	"github.com/google/uuid"
	"github.com/jackc/pgx/v5"
)

var (
	FactsBudget = uuid.MustParse("00000000-0000-0000-0000-000000000001")
	ReapsBudget = uuid.MustParse("00000000-0000-0000-0000-000000000002")
)

func (store *Store) EnsureBudget(operation context.Context, key uuid.UUID, rate float64, burst int) error {
	_, failure := store.pool.Exec(operation, `insert into campaign_budgets (campaign_id, rate, burst) values ($1, $2, $3)
		on conflict (campaign_id) do update set rate = excluded.rate, burst = excluded.burst
		where (campaign_budgets.rate, campaign_budgets.burst) is distinct from (excluded.rate, excluded.burst)`, key, rate, burst)
	return failure
}

func (store *Store) LeaseTokens(operation context.Context, key uuid.UUID, wanted int, running bool) (int, error) {
	var granted int
	failure := store.pool.QueryRow(operation, `with budget as (
			select least(burst, tokens + greatest(0, extract(epoch from clock_timestamp() - refilled_at)) * rate) as available, clock_timestamp() as at
			from campaign_budgets where campaign_id = $1 for update
		)
		update campaign_budgets set tokens = budget.available - least($2, floor(budget.available)), refilled_at = greatest(campaign_budgets.refilled_at, budget.at)
		from budget
		where campaign_budgets.campaign_id = $1 and budget.available >= 1
			and (not $3 or exists (select 1 from campaigns where id = $1 and status = 'running'))
		returning least($2, floor(budget.available))::integer`, key, wanted, running).Scan(&granted)
	if errors.Is(failure, pgx.ErrNoRows) {
		return 0, nil
	}
	return granted, failure
}

func (store *Store) ReturnTokens(operation context.Context, key uuid.UUID, count int) error {
	_, failure := store.pool.Exec(operation, `update campaign_budgets set tokens = least(burst, tokens + $2) where campaign_id = $1`, key, count)
	return failure
}
