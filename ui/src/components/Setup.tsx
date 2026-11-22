import { useState } from 'react'

import { api } from '@/api'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { run } from '@/store'

/** First launch: choose a display name. */
export function Setup() {
  const [name, setName] = useState('')

  return (
    <section className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center">
      <img src="/icon.svg" alt="" className="size-18" />
      <h1 className="text-lg font-semibold">Tightbeam</h1>
      <p className="text-sm text-muted-foreground">
        A private line between two people on the same network.
      </p>
      <form
        className="mt-3 flex w-full max-w-72 flex-col gap-3 text-left"
        onSubmit={(e) => {
          e.preventDefault()
          void run(api.setName(name))
        }}
      >
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="name">Your name</Label>
          <Input
            id="name"
            maxLength={40}
            autoFocus
            required
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </div>
        <Button type="submit">Continue</Button>
      </form>
    </section>
  )
}
