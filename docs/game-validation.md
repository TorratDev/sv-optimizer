# Validation on a real farm

The project is observe-and-advise only. These checks validate observations and calculations against a real unmodded 1.6.x farm; the developer workspace does not contain Stardew Valley.

1. Install SMAPI and the observer; launch a single-player save. Confirm the detected game/SMAPI versions and no observation errors in the SMAPI console.
2. Start `sv-optimizer chat --no-model`, then `/snapshot`. Compare money, date, skills, inventory, chest contents, watering can, crops and fertilizer to the game. Check that only accessible outdoor main-farm land is offered.
3. Confirm sprinkler-covered tiles and currently watered tiles independently. Include pressure-nozzle coverage if present. Newly tilled soil must still be watered that day.
4. Check the seed prices and availability at Pierre's, Joja, Sandy, Clint and Krobus where unlocked. Check Wednesday, festival and Friday closures/stock. Unsupported dynamic entries must be omitted with assumptions; they must never be invented.
5. Generate a short plan. Compare seed cost, first harvest dates and regrowth dates to the game. Test an existing Speed-Gro/Agriculturist plot separately. Check minimum and quality selling prices, including a reduced-profit-margin farm if used.
6. Follow one day manually. Sell directly and confirm immediate money; ship another crop and confirm the next-morning payment. `/progress` should explain differences and leave the saved plan unchanged until `/plan` is requested.
7. Place a sprinkler manually and refresh. Confirm that the plan does not assume it watered on the placement day. Test crafting inputs and a purchased sprinkler when available.
8. Test season-end harvests with the deadline on day 28. Shipping on day 28 must not count toward end-of-day cash; direct sales must fit shop hours and travel estimates.
9. Return to the title. Live tools must reject planning. Restart server/game and reconnect; historical plans should remain in SQLite and the observer should reload the new bridge token.
10. Run `doctor --hardware-only` with the game open, explicitly download a recommended model, then run the tool-use check. Confirm natural-language recommendations use the same calculations as `/plan`.

Record OS, game version, SMAPI version, observed differences and which assumptions needed overrides. A successful synthetic/contract test is not evidence that this checklist has passed.
