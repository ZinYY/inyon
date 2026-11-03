// applies the theme before first paint to avoid a flash (a file, not inline, so the page's CSP
// can refuse inline scripts)
try {
  var t = JSON.parse((localStorage.getItem('inyon.settings.v2') || localStorage.getItem('tether.settings.v2')) || '{}').theme || 'system'
  if (t === 'system') t = matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
  document.documentElement.dataset.theme = t
  // the ground before the stylesheet arrives, so nothing white shows first
  document.documentElement.style.background = t === 'dark' ? '#1b1a18' : '#e9e8e4'
} catch (e) {}
