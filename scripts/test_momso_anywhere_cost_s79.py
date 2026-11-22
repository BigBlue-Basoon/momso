import unittest

from momso_anywhere_cost_s79 import estimate


class CostEstimateTests(unittest.TestCase):
    def test_daily_scenarios(self):
        self.assertAlmostEqual(sum(estimate(30, 9000).values()), 5.5475, places=3)
        self.assertAlmostEqual(sum(estimate(240, 72000).values()), 10.3547, places=3)
        self.assertAlmostEqual(sum(estimate(720, 216000).values()), 24.2802, places=3)

    def test_nat_ip_assumption_is_explicit(self):
        continuous = estimate(30, 9000)
        released = estimate(30, 9000, nat_ip_hours=30)
        self.assertAlmostEqual(continuous['nat_ip'] - released['nat_ip'], 3.45)

    def test_invalid_usage(self):
        with self.assertRaises(ValueError):
            estimate(721, 0)
        with self.assertRaises(ValueError):
            estimate(0, -1)
        with self.assertRaises(ValueError):
            estimate(0, 0, korea_gib=1025)


if __name__ == '__main__':
    unittest.main()
