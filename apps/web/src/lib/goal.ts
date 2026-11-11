/** Preserve unfamiliar native states instead of presenting them as active or cleared. */
export function goalStatus(status: string): string {
  return ({ active: 'Active', paused: 'Paused', blocked: 'Blocked', complete: 'Complete', budget_limited: 'Budget limit reached', usage_limited: 'Usage limit reached' } as Record<string, string>)[status]
    ?? status.replace(/[_-]/g, ' ').replace(/^./, (c) => c.toUpperCase())
}

export function goalSummary(goal: { condition: string; status?: string }): string {
  return goal.status ? `${goalStatus(goal.status)}: ${goal.condition}` : goal.condition
}
