#!/usr/bin/env python3
"""List LemonSqueezy variants, or emit the SQL that wires them to our rows.

Reads a LemonSqueezy `GET /v1/variants?include=product` response (a file, or
stdin) so the API is called once and the result can be re-parsed freely.

    # 1. fetch once
    curl -sS "https://api.lemonsqueezy.com/v1/variants?include=product&page[size]=100" \
      -H 'Accept: application/vnd.api+json' \
      -H 'Content-Type: application/vnd.api+json' \
      -H "Authorization: Bearer $LS_API_KEY" > variants.json

    # 2. see what is there
    python3 server/scripts/ls_variants.py variants.json

    # 3. once MAP below is filled in, emit the UPDATEs
    python3 server/scripts/ls_variants.py variants.json --sql > seed_ls_variants_live.sql

Generating the SQL beats transcribing ids by hand: a mistyped variant id does
not fail loudly, it silently sells the wrong plan (or, on the webhook side,
takes a customer's money and credits nothing).
"""

import json
import sys

# ("Product name", "Variant name") in LemonSqueezy  ->  (our table, our row id).
# Both names must match the dashboard EXACTLY; anything unmatched is reported.
# The 14 rows that need an id: 6 in `plans`, 8 in `credit_packs`.
MAP: dict[tuple[str, str], tuple[str, str]] = {
    # Subscriptions. NB: each product also has a `status: pending` "Default"
    # variant that LemonSqueezy auto-creates — those are NOT purchasable and
    # must not be used here.
    ("Dragent Pro", "Monthly"): ("plans", "pro-monthly"),
    ("Dragent Pro", "Annual (Save 10%)"): ("plans", "pro-annual"),
    ("Dragent Max", "Monthly"): ("plans", "max-monthly"),
    ("Dragent Max", "Annual (Save 10%)"): ("plans", "max-annual"),
    ("Dragent Ultra", "Monthly"): ("plans", "ultra-monthly"),
    ("Dragent Ultra", "Annual (Save 10%)"): ("plans", "ultra-annual"),
    # One-time credit packs, all variants of the single "Dragent credits"
    # product. `pack-5` has no live variant — see the note in the runbook.
    ("Dragent credits", "$10 credits"): ("credit_packs", "pack-10"),
    ("Dragent credits", "$20 credits"): ("credit_packs", "pack-20"),
    ("Dragent credits", "$50 credits"): ("credit_packs", "pack-50"),
    ("Dragent credits", "$100 credits"): ("credit_packs", "pack-100"),
    ("Dragent credits", "$200 credits"): ("credit_packs", "pack-200"),
    ("Dragent credits", "$500 credits"): ("credit_packs", "pack-500"),
    ("Dragent credits", "$1000 credits"): ("credit_packs", "pack-1000"),
}


def load(path: str | None) -> dict:
    if path and path != "-":
        with open(path, encoding="utf-8") as fh:
            return json.load(fh)
    return json.load(sys.stdin)


def rows(doc: dict):
    """Yield (variant_id, product_name, variant_name, price_cents)."""
    products = {
        p["id"]: p["attributes"]["name"]
        for p in doc.get("included", [])
        if p.get("type") == "products"
    }
    for v in doc.get("data", []):
        attrs = v.get("attributes", {})
        rel = v.get("relationships", {}).get("product", {}).get("data") or {}
        yield (
            v.get("id", ""),
            products.get(rel.get("id"), "?"),
            attrs.get("name", ""),
            attrs.get("price"),
        )


def money(cents) -> str:
    return f"${cents / 100:,.2f}" if isinstance(cents, int) else "-"


def main() -> int:
    args = [a for a in sys.argv[1:] if a != "--sql"]
    want_sql = "--sql" in sys.argv[1:]
    doc = load(args[0] if args else None)
    listed = list(rows(doc))

    if not want_sql:
        print(f"{'VARIANT ID':>12}  {'PRODUCT':<34} {'VARIANT':<26} PRICE")
        print(f"{'-' * 12}  {'-' * 34} {'-' * 26} {'-' * 10}")
        for vid, product, variant, price in listed:
            print(f"{vid:>12}  {product[:33]:<34} {variant[:25]:<26} {money(price)}")
        print(f"\n{len(listed)} variant(s). Fill MAP in this script, then re-run with --sql.")
        return 0

    if not MAP:
        print("MAP is empty — fill it in before using --sql.", file=sys.stderr)
        return 2

    seen = set()
    for vid, product, variant, _ in listed:
        key = (product, variant)
        if key in MAP:
            table, row_id = MAP[key]
            seen.add(key)
            print(f"UPDATE {table} SET lemonsqueezy_variant_id = '{vid}' WHERE id = '{row_id}';")

    # An unmatched entry means a name typo — surface it instead of emitting
    # SQL that quietly leaves a purchasable row unwired.
    missing = sorted(set(MAP) - seen)
    for key in missing:
        print(f"-- UNMATCHED in LemonSqueezy: {key} -> {MAP[key]}", file=sys.stderr)
    if missing:
        print(f"\n{len(missing)} row(s) unmatched — see stderr.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
