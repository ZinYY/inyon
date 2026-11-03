import { expect, test } from 'bun:test'
import { redact } from './redact.ts'

// made up, and put together here so no scanner mistakes the source for a real token
const TOKEN = ['tk', '9f8e7d6c5b4a39281706f5e4d3c2b1a0'].join('_')
const SECRETS = [
  TOKEN,
  'abc123ticketvalue',
  'K7Q4-MX2T 493812',
  'K7Q4MX2T:493812',
  'hunter2',
  's3ss10n-c00kie-value',
  'yj4bqhvahk8dpwnc7x7ip3rr5n6fgw1zufw9idmd6a6pkopzxx5o',
]

test('no secret survives', () => {
  const text = [
    'wss://hub.example:4777/ws?ticket=abc123ticketvalue&catalog=3',
    `authorization: ${'Bear' + 'er'} ${TOKEN}`,
    'Cookie: inyon_session=s3ss10n-c00kie-value; theme=dark',
    'set the code K7Q4-MX2T 493812 or K7Q4MX2T:493812',
    '{"password": "hunter2"}',
    'inyon://pair?m=yj4bqhvahk8dpwnc7x7ip3rr5n6fgw1zufw9idmd6a6pkopzxx5o&s=abc',
    'https://inyon.tech/download/#hub?u=https://box&c=K7Q4MX2T:493812',
    'https://relay.pkarr.org/yj4bqhvahk8dpwnc7x7ip3rr5n6fgw1zufw9idmd6a6pkopzxx5o: 404',
  ].join('\n')
  const out = redact(text)
  for (const s of SECRETS) expect(out).not.toContain(s)
})

test('addresses, words and paths stay readable', () => {
  const text = 'Couldn\'t connect to hub.example:4777. https://hub.example/api/session answered 421. close code 1006 at /home/sam/development/project-folder'
  expect(redact(text)).toBe(text)
  expect(redact('wss://hub.example/ws?ticket=x')).toBe('wss://hub.example/ws?[hidden]')
  expect(redact('Bearer abc')).toBe('Bearer [hidden]')
  expect(redact('inyon://hub?u=x&c=y')).toBe('inyon://hub?[hidden]')
})

test('a quoted secret goes whole, spaces and all', () => {
  const out = redact('password: "correct horse battery staple" then token=\'two words\' end')
  expect(out).toBe('password: [hidden] then token=[hidden] end')
})

test('a private key block goes whole', () => {
  const key = '-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----'
  expect(redact(`key:\n${key}\nafter`)).not.toContain('b3BlbnNzaC1rZXktdjEAAAAA')
})
