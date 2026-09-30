# pageindex-rs vs Python PageIndex @619cbd8: FinanceBench, LLM-free

Sandbox run on a 4-core container. Python extraction is serial, 3 docs concurrently; Rust is single process. Pipeline: extract → tree → merge optimize, no LLM.

**Final trees (structure, toc_source, doc_title, optimize report) are identical on 84/84 filings.**


| metric | Python | Rust |
|---|---|---|
| docs ok | 84/84 | 84/84 |
| pages | 12013 | 12013 |
| extraction s | 2204.7 | 182.8 |
| ms/page (pooled) | 183.5 | 15.2 |
| nodes (mean) | 86.9 | 86.9 |
| evidence span p50 | 5 | 5 |
| evidence span <= 3p | 45.5% | 45.5% |
| evidence span <= 10p | 60.8% | 60.8% |

Docs with identical node count and toc_source: 84/84

## Per document

| doc | pages | Py ms/p | Rust ms/p | speedup | Py nodes | Rust nodes | toc Py/Rust |
|---|---|---|---|---|---|---|---|
| 3M_2018_10K | 160 | 473.9 | 26.2 | 18.1x | 50 | 50 | detected/detected |
| 3M_2022_10K | 252 | 432.8 | 26.9 | 16.1x | 65 | 65 | detected/detected |
| 3M_2023Q2_10Q | 92 | 462.4 | 29.8 | 15.5x | 13 | 13 | detected/detected |
| ACTIVISIONBLIZZARD_2019_10K | 198 | 263.1 | 19.4 | 13.6x | 23 | 23 | detected/detected |
| ADOBE_2015_10K | 116 | 130.7 | 11.3 | 11.6x | 126 | 126 | bookmarks/bookmarks |
| ADOBE_2016_10K | 112 | 123.3 | 11.5 | 10.7x | 75 | 75 | bookmarks/bookmarks |
| ADOBE_2017_10K | 107 | 119.1 | 13.9 | 8.6x | 74 | 74 | bookmarks/bookmarks |
| ADOBE_2022_10K | 99 | 68.4 | 10.8 | 6.3x | 78 | 78 | bookmarks/bookmarks |
| AES_2022_10K | 257 | 352.7 | 24.2 | 14.6x | 220 | 220 | detected/detected |
| AMAZON_2017_10K | 85 | 117.4 | 8.5 | 13.8x | 27 | 27 | detected/detected |
| AMAZON_2019_10K | 83 | 92.2 | 8.4 | 11.0x | 23 | 23 | detected/detected |
| AMCOR_2020_10K | 195 | 84.8 | 13.1 | 6.5x | 156 | 156 | detected/detected |
| AMCOR_2022_8K_dated-2022-07-01 | 9 | 258.9 | 20.9 | 12.4x | 10 | 10 | detected/detected |
| AMCOR_2023Q2_10Q | 57 | 180.8 | 12.8 | 14.1x | 52 | 52 | detected/detected |
| AMCOR_2023Q4_EARNINGS | 14 | 111.2 | 10.9 | 10.2x | 9 | 9 | hybrid/hybrid |
| AMCOR_2023_10K | 156 | 286.8 | 21.7 | 13.2x | 146 | 146 | detected/detected |
| AMD_2015_10K | 215 | 286.2 | 18.5 | 15.5x | 189 | 189 | detected/detected |
| AMD_2022_10K | 121 | 313.3 | 24.8 | 12.6x | 25 | 25 | detected/detected |
| AMERICANEXPRESS_2022_10K | 260 | 247.3 | 18.9 | 13.1x | 84 | 84 | detected/detected |
| AMERICANWATERWORKS_2020_10K | 152 | 110.2 | 10.1 | 10.9x | 38 | 38 | detected/detected |
| AMERICANWATERWORKS_2021_10K | 181 | 86.0 | 10.1 | 8.5x | 46 | 46 | detected/detected |
| AMERICANWATERWORKS_2022_10K | 306 | 211.4 | 18.0 | 11.7x | 73 | 73 | detected/detected |
| BESTBUY_2017_10K | 109 | 87.0 | 11.5 | 7.6x | 30 | 30 | detected/detected |
| BESTBUY_2019_10K | 107 | 86.3 | 11.4 | 7.6x | 15 | 15 | detected/detected |
| BESTBUY_2023_10K | 75 | 158.2 | 21.7 | 7.3x | 83 | 83 | detected/detected |
| BESTBUY_2024Q2_10Q | 30 | 163.4 | 22.5 | 7.3x | 11 | 11 | detected/detected |
| BLOCK_2016_10K | 121 | 92.4 | 10.8 | 8.6x | 142 | 142 | detected/detected |
| BLOCK_2020_10K | 158 | 91.6 | 9.0 | 10.2x | 159 | 159 | detected/detected |
| BOEING_2018_10K | 136 | 82.3 | 8.1 | 10.2x | 25 | 25 | detected/detected |
| BOEING_2022_10K | 190 | 201.1 | 15.9 | 12.6x | 58 | 58 | detected/detected |
| COCACOLA_2017_10K | 197 | 391.0 | 29.4 | 13.3x | 170 | 170 | detected/detected |
| COCACOLA_2021_10K | 183 | 320.1 | 24.9 | 12.9x | 166 | 166 | detected/detected |
| COCACOLA_2022_10K | 183 | 326.2 | 26.4 | 12.4x | 150 | 150 | detected/detected |
| CORNING_2020_10K | 138 | 125.7 | 11.4 | 11.0x | 112 | 112 | detected/detected |
| CORNING_2021_10K | 125 | 101.0 | 10.5 | 9.6x | 30 | 30 | detected/detected |
| CORNING_2022_10K | 159 | 234.6 | 19.5 | 12.0x | 34 | 34 | detected/detected |
| COSTCO_2021_10K | 76 | 244.9 | 17.9 | 13.7x | 22 | 22 | detected/detected |
| CVSHEALTH_2018_10K | 392 | 85.9 | 8.7 | 9.9x | 281 | 281 | detected/detected |
| CVSHEALTH_2022_10K | 213 | 366.7 | 27.2 | 13.5x | 199 | 199 | detected/detected |
| FOOTLOCKER_2022_8K_dated-2022-05-20 | 4 | 137.3 | 19.5 | 7.0x | 4 | 4 | detected/detected |
| FOOTLOCKER_2022_8K_dated_2022-08-19 | 31 | 278.9 | 19.9 | 14.0x | 5 | 5 | detected/detected |
| GENERALMILLS_2019_10K | 140 | 94.5 | 8.7 | 10.9x | 26 | 26 | detected/detected |
| GENERALMILLS_2020_10K | 127 | 109.2 | 14.3 | 7.6x | 135 | 135 | detected/detected |
| GENERALMILLS_2022_10K | 127 | 320.8 | 23.5 | 13.7x | 133 | 133 | detected/detected |
| JOHNSON_JOHNSON_2022Q4_EARNINGS | 24 | 106.3 | 12.8 | 8.3x | 8 | 8 | detected/detected |
| JOHNSON_JOHNSON_2022_10K | 151 | 298.7 | 22.0 | 13.6x | 130 | 130 | detected/detected |
| JOHNSON_JOHNSON_2023Q2_EARNINGS | 24 | 151.3 | 14.9 | 10.2x | 7 | 7 | detected/detected |
| JOHNSON_JOHNSON_2023_8K_dated-2023-08-30 | 27 | 192.6 | 16.4 | 11.7x | 10 | 10 | detected/detected |
| JPMORGAN_2021Q1_10Q | 179 | 103.3 | 10.4 | 9.9x | 186 | 186 | bookmarks/bookmarks |
| JPMORGAN_2022Q2_10Q | 197 | 112.4 | 10.7 | 10.5x | 204 | 204 | bookmarks/bookmarks |
| JPMORGAN_2022_10K | 382 | 115.7 | 11.3 | 10.2x | 386 | 386 | bookmarks/bookmarks |
| JPMORGAN_2023Q2_10Q | 217 | 111.5 | 10.3 | 10.8x | 219 | 219 | bookmarks/bookmarks |
| KRAFTHEINZ_2019_10K | 152 | 331.5 | 24.5 | 13.5x | 148 | 148 | detected/detected |
| LOCKHEEDMARTIN_2020_10K | 132 | 342.6 | 28.1 | 12.2x | 20 | 20 | detected/detected |
| LOCKHEEDMARTIN_2021_10K | 136 | 362.2 | 27.7 | 13.1x | 35 | 35 | detected/detected |
| LOCKHEEDMARTIN_2022_10K | 130 | 102.3 | 10.9 | 9.4x | 63 | 63 | bookmarks/bookmarks |
| MGMRESORTS_2018_10K | 188 | 110.6 | 12.5 | 8.8x | 137 | 137 | detected/detected |
| MGMRESORTS_2020_10K | 150 | 109.1 | 12.5 | 8.7x | 142 | 142 | detected/detected |
| MGMRESORTS_2022Q4_EARNINGS | 15 | 109.6 | 15.9 | 6.9x | 8 | 8 | detected/detected |
| MGMRESORTS_2022_10K | 220 | 248.5 | 19.9 | 12.5x | 205 | 205 | detected/detected |
| MGMRESORTS_2023Q2_10Q | 62 | 236.7 | 17.5 | 13.5x | 56 | 56 | detected/detected |
| MICROSOFT_2016_10K | 121 | 129.8 | 11.6 | 11.2x | 18 | 18 | detected/detected |
| MICROSOFT_2023_10K | 116 | 317.8 | 26.1 | 12.2x | 111 | 111 | detected/detected |
| NETFLIX_2015_10K | 72 | 81.7 | 8.7 | 9.4x | 18 | 18 | detected/detected |
| NETFLIX_2017_10K | 73 | 85.7 | 10.1 | 8.5x | 18 | 18 | detected/detected |
| NIKE_2018_10K | 97 | 110.3 | 11.7 | 9.4x | 27 | 27 | detected/detected |
| NIKE_2019_10K | 104 | 104.5 | 8.5 | 12.3x | 31 | 31 | detected/detected |
| NIKE_2021_10K | 109 | 102.6 | 7.7 | 13.3x | 27 | 27 | detected/detected |
| NIKE_2023_10K | 106 | 90.4 | 8.5 | 10.6x | 137 | 137 | hybrid/hybrid |
| PAYPAL_2022_10K | 134 | 320.3 | 24.2 | 13.2x | 41 | 41 | detected/detected |
| PEPSICO_2021_10K | 549 | 59.1 | 5.9 | 10.0x | 107 | 107 | detected/detected |
| PEPSICO_2022_10K | 503 | 69.3 | 7.5 | 9.2x | 132 | 132 | detected/detected |
| PEPSICO_2023Q1_EARNINGS | 16 | 61.1 | 11.0 | 5.6x | 14 | 14 | hybrid/hybrid |
| PEPSICO_2023_8K_dated-2023-05-05 | 5 | 107.5 | 11.4 | 9.4x | 1 | 1 | detected/detected |
| PEPSICO_2023_8K_dated-2023-05-30 | 172 | 288.6 | 21.5 | 13.4x | 78 | 78 | detected/detected |
| PFIZER_2021_10K | 143 | 139.9 | 11.3 | 12.4x | 124 | 124 | detected/detected |
| Pfizer_2023Q2_10Q | 72 | 340.9 | 23.5 | 14.5x | 54 | 54 | detected/detected |
| ULTABEAUTY_2023Q4_EARNINGS | 9 | 201.5 | 17.7 | 11.4x | 10 | 10 | detected/detected |
| ULTABEAUTY_2023_10K | 105 | 272.3 | 21.5 | 12.7x | 22 | 22 | detected/detected |
| VERIZON_2021_10K | 120 | 96.5 | 11.0 | 8.8x | 155 | 155 | bookmarks/bookmarks |
| VERIZON_2022_10K | 124 | 96.7 | 10.8 | 9.0x | 141 | 141 | bookmarks/bookmarks |
| WALMART_2018_10K | 304 | 77.5 | 7.3 | 10.6x | 214 | 214 | detected/detected |
| WALMART_2019_10K | 155 | 86.0 | 8.8 | 9.8x | 108 | 108 | detected/detected |
| WALMART_2020_10K | 170 | 93.9 | 8.6 | 10.9x | 159 | 159 | detected/detected |
