import { expect, test } from 'bun:test'

import { namer, peopleKey } from './people'

const people = [
  { id: 'bob', name: 'Bob' },
  { id: 'carol', name: 'Carol' },
]

test('each author keeps their own name', () => {
  const nameOf = namer(peopleKey(people), 'me', 'Carol')
  expect(['me', 'bob', 'carol'].map(nameOf)).toEqual(['You', 'Bob', 'Carol'])
})

test('anyone unknown gets the fallback', () => {
  expect(namer(peopleKey(undefined), 'me', 'Them')('stranger')).toBe('Them')
  expect(namer(peopleKey([{ id: 'x', name: '' }]), 'me', 'Them')('x')).toBe('Them')
})

test('the key changes only when the names do', () => {
  const copy = people.map((p) => ({ ...p }))
  expect(peopleKey(copy)).toBe(peopleKey(people))
  expect(peopleKey([people[0], { id: 'carol', name: 'C' }])).not.toBe(peopleKey(people))
})
