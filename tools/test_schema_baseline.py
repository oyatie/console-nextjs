import unittest

from schema_baseline import canonical_dump


class SchemaBaselineTests(unittest.TestCase):
    def test_dump_normalization_preserves_sql_inside_function_bodies(self):
        dump = """-- Dumped by pg_dump
\\restrict nonce
SET statement_timeout = 0;
-- Name: example; Type: FUNCTION
CREATE FUNCTION example() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
SET statement_timeout = 0;
END;
$$;
SET default_tablespace = '';
SET default_table_access_method = heap;
\\unrestrict nonce
"""
        result = canonical_dump(dump)
        self.assertEqual(result.count("SET statement_timeout = 0;"), 1)
        self.assertIn("BEGIN\nSET statement_timeout = 0;\nEND;", result)
        self.assertNotIn("nonce", result)


if __name__ == "__main__":
    unittest.main()
