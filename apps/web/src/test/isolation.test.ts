import { expect, test } from 'bun:test'

// Bun keeps a spyOn() mock in place for every test file that runs after it, in whatever order it
// picks, so a mock left behind changes what later files test.
test('a test file that mocks something puts it back when it ends', async () => {
  const src = `${import.meta.dir}/..`
  const leaks: string[] = []
  for await (const file of new Bun.Glob('**/*.test.{ts,tsx}').scan(src)) {
    const text = await Bun.file(`${src}/${file}`).text()
    if (/\bspyOn\(/.test(text) && !text.includes('afterAll(() => mock.restore())')) leaks.push(file)
  }
  expect(leaks).toEqual([])
})
