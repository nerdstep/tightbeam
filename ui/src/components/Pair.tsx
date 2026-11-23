import { shallow, useSelector } from '@tanstack/react-store'
import { useState } from 'react'

import { api } from '@/api'
import { SettingsMenu } from '@/components/SettingsMenu'
import { Button } from '@/components/ui/button'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Input } from '@/components/ui/input'
import { Item, ItemActions, ItemContent, ItemGroup, ItemTitle } from '@/components/ui/item'
import { Spinner } from '@/components/ui/spinner'
import { run, store } from '@/store'

/** Not paired yet: show who's nearby. */
export function Pair() {
  const { myName, nearby, busy } = useSelector(
    store,
    (s) => ({
      myName: s.status?.me.name ?? '',
      nearby: s.status?.nearby ?? [],
      busy: !!s.status?.pairing,
    }),
    { compare: shallow },
  )

  return (
    <section className="flex h-full flex-col">
      <header className="flex items-center justify-between gap-3 border-b bg-card px-4 py-3">
        <h1 className="font-semibold">Pair</h1>
        <div className="flex items-center gap-1">
          <Rename name={myName} />
          <SettingsMenu />
        </div>
      </header>
      {nearby.length === 0 ? (
        <Empty>
          <EmptyHeader>
            <EmptyMedia>
              <Spinner className="size-5" />
            </EmptyMedia>
            <EmptyTitle>Looking for people nearby</EmptyTitle>
            <EmptyDescription>
              Ask the other person to open Tightbeam on the same network. They’ll appear here.
            </EmptyDescription>
          </EmptyHeader>
        </Empty>
      ) : (
        <div className="flex flex-col gap-3 p-4">
          <p className="text-sm text-muted-foreground">Nearby on this network:</p>
          <ItemGroup className="gap-2">
            {nearby.map((p) => (
              <Item key={p.id} variant="outline" className="bg-card">
                <ItemContent>
                  <ItemTitle>{p.name || 'Unnamed'}</ItemTitle>
                </ItemContent>
                <ItemActions>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy}
                    onClick={() => void run(api.pairWith(p.id))}
                  >
                    Pair
                  </Button>
                </ItemActions>
              </Item>
            ))}
          </ItemGroup>
        </div>
      )}
    </section>
  )
}

/** "You're Alice · change", turning into an inline field. */
function Rename({ name }: { name: string }) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(name)

  if (!editing) {
    return (
      <span className="text-sm text-muted-foreground">
        You’re <b className="font-medium text-foreground">{name}</b> ·{' '}
        <Button
          variant="link"
          size="sm"
          className="h-auto px-0"
          onClick={() => {
            setDraft(name)
            setEditing(true)
          }}
        >
          change
        </Button>
      </span>
    )
  }

  return (
    <form
      className="flex items-center gap-2"
      onSubmit={async (e) => {
        e.preventDefault()
        await run(api.setName(draft))
        setEditing(false)
      }}
    >
      <Input
        className="h-7 w-36"
        maxLength={40}
        autoFocus
        required
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => e.key === 'Escape' && setEditing(false)}
      />
      <Button type="submit" size="sm">
        Save
      </Button>
    </form>
  )
}
