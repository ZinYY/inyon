import { describe, expect, test } from 'bun:test'
import { LOCATION, rewriteLocation } from './jsloc.ts'

const L = LOCATION

describe("a page's scripts read the page's own address", () => {
  test('location as a name, and through window, self, globalThis and document', () => {
    expect(rewriteLocation("location.href = 'list.html' + location.search")).toBe(`${L}.href = 'list.html' + ${L}.search`)
    expect(rewriteLocation('if (window.location.pathname.endsWith(x)) go()')).toBe(`if (window.${L}.pathname.endsWith(x)) go()`)
    expect(rewriteLocation('self.location.hash; globalThis.location; document.location.reload()')).toBe(`self.${L}.hash; globalThis.${L}; document.${L}.reload()`)
    expect(rewriteLocation('new URLSearchParams(location.search)')).toBe(`new URLSearchParams(${L}.search)`)
    expect(rewriteLocation('location = "b.html"')).toBe(`${L} = "b.html"`)
    expect(rewriteLocation('a ? location : b')).toBe(`a ? ${L} : b`)
    expect(rewriteLocation('typeof location')).toBe(`typeof ${L}`)
    expect(rewriteLocation('window?.location?.href')).toBe(`window?.${L}?.href`)
  })

  test("leaves anything else called location alone: properties, keys, declarations", () => {
    for (const src of ['a.location', 'a.window.location', 'item.location.city', '({ location: 1 })', '({ a, location: 2 })', 'let location = 1', 'function location() {}', 'parent.location', 'top.location.href']) {
      expect(rewriteLocation(src)).toBe(src)
    }
  })

  test('skips strings, template text, comments and regular expressions, but not code inside a template', () => {
    expect(rewriteLocation("'location.href'; \"location\"")).toBe("'location.href'; \"location\"")
    expect(rewriteLocation('// location.href = x\n/* location */ y')).toBe('// location.href = x\n/* location */ y')
    expect(rewriteLocation('`at location ${location.pathname} and ${ {a: location}.a }`')).toBe(`\`at location \${${L}.pathname} and \${ {a: ${L}}.a }\``)
    expect(rewriteLocation('x = /location\\/[/]/g.test(s); y = location')).toBe(`x = /location\\/[/]/g.test(s); y = ${L}`)
    expect(rewriteLocation('a = b / location.length / 2')).toBe(`a = b / ${L}.length / 2`)
    expect(rewriteLocation('return /location/.test(location.href)')).toBe(`return /location/.test(${L}.href)`)
    expect(rewriteLocation("s = 'it\\'s location'; location")).toBe(`s = 'it\\'s location'; ${L}`)
  })

  test('a script without the word is passed back as it is', () => {
    const src = 'const a = 1; /* nothing */'
    expect(rewriteLocation(src)).toBe(src)
  })

  test("Blueprint's own lines", () => {
    expect(rewriteLocation("r.onclick = () => (location.href = 'list.html' + location.search)")).toBe(`r.onclick = () => (${L}.href = 'list.html' + ${L}.search)`)
    expect(rewriteLocation("if (location.pathname.endsWith('/' + h)) return K.toast('You are already here'); location.href = h + location.search;")).toBe(
      `if (${L}.pathname.endsWith('/' + h)) return K.toast('You are already here'); ${L}.href = h + ${L}.search;`,
    )
    expect(rewriteLocation('const x = new URL(u, location.href); return x.origin === location.origin')).toBe(`const x = new URL(u, ${L}.href); return x.origin === ${L}.origin`)
  })
})
