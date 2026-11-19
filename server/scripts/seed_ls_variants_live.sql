-- Live LemonSqueezy variant wiring for the production database.
-- Generated from GET /v1/variants (store 432789) via server/scripts/ls_variants.py.
-- NOT a goose migration on purpose: these ids are live-store-specific and must
-- never replay into a staging database. Run once, against production only.

BEGIN;

  UPDATE credit_packs SET lemonsqueezy_variant_id = '1974664' WHERE id = 'pack-10';
  UPDATE credit_packs SET lemonsqueezy_variant_id = '1974623' WHERE id = 'pack-20';
  UPDATE credit_packs SET lemonsqueezy_variant_id = '1974656' WHERE id = 'pack-50';
  UPDATE plans SET lemonsqueezy_variant_id = '1974773' WHERE id = 'pro-monthly';
  UPDATE plans SET lemonsqueezy_variant_id = '1974793' WHERE id = 'max-monthly';
  UPDATE plans SET lemonsqueezy_variant_id = '1974804' WHERE id = 'ultra-monthly';
  UPDATE credit_packs SET lemonsqueezy_variant_id = '1974635' WHERE id = 'pack-100';
  UPDATE plans SET lemonsqueezy_variant_id = '1974785' WHERE id = 'pro-annual';
  UPDATE plans SET lemonsqueezy_variant_id = '1974797' WHERE id = 'max-annual';
  UPDATE plans SET lemonsqueezy_variant_id = '1974810' WHERE id = 'ultra-annual';
  UPDATE credit_packs SET lemonsqueezy_variant_id = '1974638' WHERE id = 'pack-200';
  UPDATE credit_packs SET lemonsqueezy_variant_id = '1974650' WHERE id = 'pack-500';
  UPDATE credit_packs SET lemonsqueezy_variant_id = '1974655' WHERE id = 'pack-1000';

  -- No $5 variant exists in the live store (smallest is $10). Deactivating
  -- rather than leaving a purchasable row that cannot check out; every plan
  -- and pack lookup filters on is_active.
  UPDATE credit_packs SET is_active = FALSE WHERE id = 'pack-5';

COMMIT;
