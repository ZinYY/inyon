// The site's only script: copy buttons, the mobile menu, the table of contents marker, search, and
// the pairing links that land on the home page.
(() => {
  const $ = (s, el = document) => el.querySelector(s)
  const $$ = (s, el = document) => [...el.querySelectorAll(s)]

  // copy buttons on code blocks and one-line commands
  for (const b of $$('.copy, .cmd button')) {
    b.addEventListener('click', async () => {
      const code = b.classList.contains('copy') ? $('pre', b.parentElement) : b.previousElementSibling
      const text = code.textContent.replace(/\n$/, '')
      try {
        await navigator.clipboard.writeText(text)
        b.textContent = 'Copied'
      } catch {
        b.textContent = 'Copy failed'
      }
      b.classList.add('done')
      setTimeout(() => {
        b.textContent = 'Copy'
        b.classList.remove('done')
      }, 1600)
    })
  }

  // mobile menu: the docs sidebar slides in; on pages without one it holds the site links
  const menu = $('[data-menu]')
  let side = $('[data-side]')
  if (!side && menu) {
    side = document.createElement('aside')
    side.className = 'side side-menu'
    side.dataset.side = ''
    side.innerHTML = `<div class="side-group"><ul>${$$('.top-nav a')
      .map((a) => `<li><a href="${a.getAttribute('href')}">${a.textContent}</a></li>`)
      .join('')}<li><a href="https://github.com/Abhash-Chakraborty/inyon" class="ext">GitHub</a></li></ul></div>`
    document.body.append(side)
  }
  const setMenu = (open) => {
    document.body.classList.toggle('menu-open', open)
    menu?.setAttribute('aria-expanded', String(open))
  }
  menu?.addEventListener('click', () => setMenu(!document.body.classList.contains('menu-open')))
  document.addEventListener('click', (e) => {
    if (document.body.classList.contains('menu-open') && !side.contains(e.target) && !menu.contains(e.target)) setMenu(false)
  })
  side?.addEventListener('click', (e) => e.target.closest('a') && setMenu(false))

  // "On this page" follows the reader
  const toc = $$('.toc a')
  if (toc.length) {
    const heads = toc.map((a) => document.getElementById(decodeURIComponent(a.hash.slice(1)))).filter(Boolean)
    const mark = () => {
      let current = heads[0]
      for (const h of heads) if (h.getBoundingClientRect().top < 120) current = h
      for (const a of toc) a.toggleAttribute('aria-current', a.hash === `#${current.id}`)
    }
    let queued = false
    addEventListener('scroll', () => {
      if (queued) return
      queued = true
      requestAnimationFrame(() => {
        queued = false
        mark()
      })
    }, { passive: true })
    mark()
  }

  // search: a small index of every page and section, fetched on first open
  const dialog = $('[data-search-dialog]')
  const input = $('input', dialog)
  const list = $('.search-results', dialog)
  let index = null
  let results = []
  let active = 0
  let opener = null
  const words = (q) => q.toLowerCase().split(/\s+/).filter(Boolean)
  const escape = (s) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c])
  const highlight = (text, q) => {
    let out = escape(text)
    for (const w of words(q)) out = out.replace(new RegExp(`(${w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')})`, 'gi'), '<mark>$1</mark>')
    return out
  }
  const snippet = (text, q) => {
    const lower = text.toLowerCase()
    const at = Math.min(...words(q).map((w) => lower.indexOf(w)).filter((i) => i >= 0), text.length)
    const start = Math.max(0, at - 40)
    return (start ? '…' : '') + text.slice(start, start + 160)
  }
  const score = (e, q) => {
    const ws = words(q)
    const h = (e.h || e.t).toLowerCase()
    const t = e.t.toLowerCase()
    const x = e.x.toLowerCase()
    let s = 0
    for (const w of ws) {
      if (h.includes(w)) s += h.startsWith(w) ? 12 : 8
      else if (t.includes(w)) s += 5
      else if (x.includes(w)) s += 2
      else return 0
    }
    if (!e.h) s += 1
    return s
  }
  const show = () => {
    const q = input.value.trim()
    if (!index) return
    results = q
      ? index
          .map((e) => [score(e, q), e])
          .filter(([s]) => s > 0)
          .sort((a, b) => b[0] - a[0])
          .slice(0, 12)
          .map(([, e]) => e)
      : []
    active = 0
    list.innerHTML = results.length
      ? results
          .map(
            (e, i) =>
              `<li><a href="${e.u}" role="option" aria-selected="${i === 0}"><span class="where">${escape(e.t)}</span><span class="what">${highlight(e.h || e.t, q)}</span><span class="snip">${highlight(snippet(e.x, q), q)}</span></a></li>`,
          )
          .join('')
      : q
        ? `<li class="search-empty">Nothing found for “${escape(q)}”.</li>`
        : ''
  }
  const move = (d) => {
    const links = $$('a', list)
    if (!links.length) return
    active = (active + d + links.length) % links.length
    links.forEach((a, i) => a.setAttribute('aria-selected', String(i === active)))
    links[active].scrollIntoView({ block: 'nearest' })
  }
  const open = async () => {
    opener = document.activeElement
    dialog.hidden = false
    document.body.style.overflow = 'hidden'
    input.focus()
    input.select()
    if (!index) {
      try {
        index = await (await fetch('/assets/search.json')).json()
      } catch {
        index = []
      }
      show()
    }
  }
  const close = () => {
    dialog.hidden = true
    document.body.style.overflow = ''
    opener?.focus?.()
  }
  for (const b of $$('[data-search]')) b.addEventListener('click', open)
  input.addEventListener('input', show)
  dialog.addEventListener('click', (e) => e.target === dialog && close())
  list.addEventListener('click', (e) => e.target.closest('a') && close())
  addEventListener('keydown', (e) => {
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName) || document.activeElement?.isContentEditable
    if (!dialog.hidden) {
      if (e.key === 'Escape') close()
      else if (e.key === 'ArrowDown') (e.preventDefault(), move(1))
      else if (e.key === 'ArrowUp') (e.preventDefault(), move(-1))
      else if (e.key === 'Enter' && results[active]) {
        e.preventDefault()
        location.href = results[active].u
        close()
      }
      return
    }
    if ((e.key === '/' && !typing) || ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k')) {
      e.preventDefault()
      open()
    } else if (e.key === 'Escape' && document.body.classList.contains('menu-open')) setMenu(false)
  })

  // the home video: the poster becomes YouTube's player only when it's played
  for (const a of $$('[data-youtube]')) {
    a.addEventListener('click', (e) => {
      if (e.metaKey || e.ctrlKey || e.shiftKey || e.button) return
      e.preventDefault()
      const frame = document.createElement('iframe')
      frame.src = `https://www.youtube-nocookie.com/embed/${a.dataset.youtube}?autoplay=1&rel=0`
      frame.title = 'Inyon in 90 seconds'
      frame.allow = 'autoplay; encrypted-media; picture-in-picture; fullscreen'
      frame.allowFullscreen = true
      a.replaceWith(frame)
    })
  }

  // an old pairing link or app address lands on the 404 page: the home page explains what to do with it
  if ($('.notfound') && /^#(pair|hub)\?/.test(location.hash)) location.replace('/' + location.hash)

  // A computer's or hub's pairing QR, scanned with the phone's own camera, opens this site with the
  // code after the #, which browsers never send to a server. Offer to copy it for the app, then
  // take it out of the address bar.
  const pair = $('#pair')
  if (pair && /^#(pair|hub)\?/.test(location.hash)) {
    const link = location.href
    pair.classList.add('on')
    if (location.hash.startsWith('#hub?')) {
      $('#pair-title').textContent = 'Pair your phone with this hub'
      $('#pair-text').innerHTML =
        'This link pairs the Inyon app with a hub. In the app, tap <b>Add</b>, then <b>Connect to a hub</b> and scan the code again, or copy this link and paste it there. It never leaves this page.'
    }
    history.replaceState(null, '', location.pathname)
    $('#copy-pair').addEventListener('click', async (e) => {
      try {
        await navigator.clipboard.writeText(link)
        e.target.textContent = 'Copied'
      } catch {
        e.target.textContent = 'Copy failed'
      }
    })
  }
})()
