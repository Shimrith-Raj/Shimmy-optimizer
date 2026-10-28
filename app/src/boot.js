// Runs before the page draws: enables entrance motion, with a safety net so content never stays hidden if app.js fails.
document.documentElement.classList.add('js');
setTimeout(() => document.querySelectorAll('.reveal').forEach((e) => e.classList.add('in')), 1500);
