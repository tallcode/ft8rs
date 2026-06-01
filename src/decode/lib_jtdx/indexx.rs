//! Mirrors JTDX `lib/indexx.f90`.

pub(crate) fn indexx_ascending(arr: &[f64]) -> Vec<usize> {
    const M: usize = 7;
    const NSTACK: usize = 50;

    let n = arr.len();
    let mut indx: Vec<usize> = (0..n).collect();
    if n == 0 {
        return indx;
    }

    let mut istack = [0usize; NSTACK];
    let mut jstack = 0usize;
    let mut l = 0usize;
    let mut ir = n - 1;

    loop {
        if ir.saturating_sub(l) < M {
            if l < ir {
                for j in (l + 1)..=ir {
                    let indxt = indx[j];
                    let a = arr[indxt];
                    let mut insert_at = 0usize;
                    for i in (0..j).rev() {
                        if arr[indx[i]] <= a {
                            insert_at = i + 1;
                            break;
                        }
                        indx[i + 1] = indx[i];
                    }
                    indx[insert_at] = indxt;
                }
            }

            if jstack == 0 {
                return indx;
            }
            ir = istack[jstack - 1];
            l = istack[jstack - 2];
            jstack -= 2;
        } else {
            let k = (l + ir) / 2;
            indx.swap(k, l + 1);

            if arr[indx[l + 1]] > arr[indx[ir]] {
                indx.swap(l + 1, ir);
            }
            if arr[indx[l]] > arr[indx[ir]] {
                indx.swap(l, ir);
            }
            if arr[indx[l + 1]] > arr[indx[l]] {
                indx.swap(l + 1, l);
            }

            let mut i = l + 1;
            let mut j = ir;
            let indxt = indx[l];
            let a = arr[indxt];
            loop {
                loop {
                    i += 1;
                    if arr[indx[i]] >= a {
                        break;
                    }
                }
                loop {
                    j -= 1;
                    if arr[indx[j]] <= a {
                        break;
                    }
                }
                if j < i {
                    break;
                }
                indx.swap(i, j);
            }

            indx[l] = indx[j];
            indx[j] = indxt;

            jstack += 2;
            assert!(jstack <= NSTACK, "NSTACK too small in indexx");
            if ir - i + 1 >= j - l {
                istack[jstack - 1] = ir;
                istack[jstack - 2] = i;
                ir = j.saturating_sub(1);
            } else {
                istack[jstack - 1] = j.saturating_sub(1);
                istack[jstack - 2] = l;
                l = i;
            }
        }
    }
}
