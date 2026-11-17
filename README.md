# Week 9 — Tastemir Shamiil

Vault на Solana / Anchor 0.31.1: депозит и share-токены. Продолжение недели 8.

- `deposit(amount)` — токены уходят в vault, vault печатает пользователю shares: первый депозит 1:1, дальше `net × total_shares / total_tokens`. Без карточки депозит невозможен.
- Комиссия 1% с депозита уходит на `fee_wallet`, который задаётся при создании vault.
- Минимальный депозит — 1 токен.
- Пока vault на паузе, депозиты запрещены.
- Трое пользователей вносят 100, 50 и 250 токенов → доли по shares 25%, 12.5% и 62.5%.
- Карточку с shares закрыть нельзя.

Program ID: `AdDmG6ALyofdymfyk9mx4vzAPFnnYqQYSi6WFmznDkGQ`

14 тестов пройдены локально и в Devnet.
