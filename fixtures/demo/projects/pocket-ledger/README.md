---
status: active
started: 2026-04-11
updated: 2026-09-19
---

# Pocket Ledger

Shared budgets for a household, without the spreadsheet. Two people add expenses from their phones and see the month at a glance.

## This month

- [x] Split an expense unevenly, 60/40 or item by item
- [x] Recurring bills
- [ ] Import a bank's CSV export
- [ ] A gentle nudge when a category runs over

## Money

Amounts are whole cents with the currency beside them, never floats:

```ruby
Money = Data.define(:cents, :currency) do
  def +(other)
    raise ArgumentError, "currencies differ" unless currency == other.currency

    with(cents: cents + other.cents)
  end

  def to_s = format("%.2f %s", cents / 100.0, currency)
end
```
