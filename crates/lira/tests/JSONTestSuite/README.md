# JSONTestSuite

These files are the folder `test_parsing` of
[JSONTestSuite](https://github.com/nst/JSONTestSuite) by Nicolas Seriot, at
commit `1ef36fa01286573e846ac449e8683f8833c5b26a`. [LICENSE](LICENSE) is its
licence. `tests/json.rs` reads each file with Lira's JSON reader.

- A reader must accept a file whose name starts with `y_`.
- A reader must refuse a file whose name starts with `n_`.
- A reader can do either with a file whose name starts with `i_`.

Two files are not here, because they are large and each is one text many
times: `n_structure_100000_opening_arrays.json` and
`n_structure_open_array_object.json`. A test in `src/json.rs` makes both.
