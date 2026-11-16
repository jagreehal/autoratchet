export function List({ items }: { items: string[] }) {
  return <ul>{items.map((i) => (i ? <li key={i}>{i}</li> : null))}</ul>;
}
