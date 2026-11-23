// The nudge sound, synthesized so there's no audio file to ship: two quick rising notes,
// soft like a chime.

let context: AudioContext | undefined

/** Each note: when it starts (seconds) and its pitch (Hz). */
const NOTES = [
  [0, 659.25], // E5
  [0.13, 987.77], // B5
] as const

export function chime() {
  try {
    context ??= new AudioContext()
    // Starts suspended until the page has had a click or a key press; then it's silent.
    void context.resume().catch(() => {})
    const start = context.currentTime + 0.02
    for (const [offset, pitch] of NOTES) {
      const at = start + offset
      // A sine, plus a quieter one an octave up for a bell-like edge.
      for (const [multiple, level] of [
        [1, 0.18],
        [2, 0.05],
      ]) {
        const tone = context.createOscillator()
        const volume = context.createGain()
        tone.type = 'sine'
        tone.frequency.setValueAtTime(pitch * multiple, at)
        volume.gain.setValueAtTime(0, at)
        volume.gain.linearRampToValueAtTime(level, at + 0.008)
        volume.gain.exponentialRampToValueAtTime(0.0001, at + 0.45)
        tone.connect(volume).connect(context.destination)
        tone.start(at)
        tone.stop(at + 0.5)
      }
    }
  } catch {
    // No audio device: the shake still happens.
  }
}
