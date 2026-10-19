;;! component_model_async = true
;;! component_model_more_async_builtins = true
;;! reference_types = true
;;! bulk_memory = true

;; $D writes two records to a stream and $C reads them, one and then the other.
;; The records have everything a copy checks or rewrites: strings, lists, bools,
;; chars, options, enums and flags, as well as padding and an unused option
;; payload, which mustn't be copied. $C fills its memory with 0x55 and then
;; returns a hash of it, covering the records and everything allocated for them.
;;
;; `run`'s first parameter says whether $D uses the `latin1+utf16` encoding,
;; which makes the copy go through `Val`s. Its strings are ASCII, so their bytes
;; are the same either way, and each assertion is made both ways. The other two
;; are an address in $D's memory and an `i32` to store there first.
(component definition $T
  (type $e (enum "x" "y" "z"))
  (type $f (flags "p" "q" "r"))
  (type $rec (record
    (field "s" string)
    (field "l" (list u16))
    (field "b" bool)
    (field "c" char)
    (field "o" (option string))
    (field "e" $e)
    (field "f" $f)
    (field "lt" (list (tuple string bool)))))

  (component $C
    (import "e" (type $e' (eq $e)))
    (import "f" (type $f' (eq $f)))
    (type $r (record
      (field "s" string)
      (field "l" (list u16))
      (field "b" bool)
      (field "c" char)
      (field "o" (option string))
      (field "e" $e')
      (field "f" $f')
      (field "lt" (list (tuple string bool)))))
    (import "rec" (type $rec' (eq $r)))
    (type $ST (stream $rec'))
    (core module $Libc
      (memory (export "mem") 1)
      (global $heap (export "heap") (mut i32) (i32.const 0x1000))
      ;; A bump allocator, which also grows memory every time.
      (func (export "realloc") (param i32 i32 i32 i32) (result i32)
        (local $p i32)
        (drop (memory.grow (i32.const 1)))
        (local.set $p (i32.and
          (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
          (i32.sub (i32.const 0) (local.get 2))))
        (global.set $heap (i32.add (local.get $p) (local.get 3)))
        (local.get $p)))
    (core instance $libc (instantiate $Libc))
    (core func $stream.read (canon stream.read $ST async
      (memory (core memory $libc "mem")) (realloc (core func $libc "realloc"))))
    (core module $M
      (import "" "mem" (memory 1))
      (import "" "heap" (global $heap (mut i32)))
      (import "" "stream.read" (func $stream.read (param i32 i32 i32) (result i32)))
      (func (export "read") (param $rx i32) (result i32)
        (local $i i32) (local $h i32)
        (memory.fill (i32.const 0) (i32.const 0x55) (i32.const 0x1100))
        ;; Read one record, then ask for two and get the other.
        (if (i32.ne (call $stream.read (local.get $rx) (i32.const 16) (i32.const 1))
              (i32.const 0x10 (; COMPLETED | (1 << 4) ;)))
          (then unreachable))
        (if (i32.ne (call $stream.read (local.get $rx) (i32.const 64) (i32.const 2))
              (i32.const 0x10))
          (then unreachable))
        (loop $next
          (local.set $h (i32.add (i32.mul (local.get $h) (i32.const 31))
            (i32.load8_u (local.get $i))))
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br_if $next (i32.lt_u (local.get $i) (global.get $heap))))
        (local.get $h)))
    (core instance $m (instantiate $M (with "" (instance
      (export "mem" (memory $libc "mem"))
      (export "heap" (global $libc "heap"))
      (export "stream.read" (func $stream.read))))))
    (func (export "read") (param "s" $ST) (result u32) (canon lift (core func $m "read")))
  )

  (component $D
    (import "e" (type $e' (eq $e)))
    (import "f" (type $f' (eq $f)))
    (type $r (record
      (field "s" string)
      (field "l" (list u16))
      (field "b" bool)
      (field "c" char)
      (field "o" (option string))
      (field "e" $e')
      (field "f" $f')
      (field "lt" (list (tuple string bool)))))
    (import "rec" (type $rec' (eq $r)))
    (type $ST (stream $rec'))
    (import "read" (func $read (param "s" $ST) (result u32)))
    (core module $Memory
      (memory (export "mem") 1)
      ;; "hello", [1, 2, 3], true (stored as 2), 'é', some("world"), z, all flags
      ;; (and more bits), [("hi", true (stored as 3)), ("", false)], with padding
      ;; of 0x77.
      (data (i32.const 16)
        "\00\01\00\00\05\00\00\00" "\10\01\00\00\03\00\00\00" "\02\77\77\77" "\e9\00\00\00"
        "\01\77\77\77\08\01\00\00\05\00\00\00" "\02\ff\77\77" "\18\01\00\00\02\00\00\00")
      ;; "ab", [7], false, 'a', none (with a payload of 0x77), x, no flags, [].
      (data (i32.const 64)
        "\32\01\00\00\02\00\00\00" "\36\01\00\00\01\00\00\00" "\00\77\77\77" "\61\00\00\00"
        "\00\77\77\77\77\77\77\77\77\77\77\77" "\00\00\77\77" "\18\01\00\00\00\00\00\00")
      (data (i32.const 0x100) "hello\00\00\00world\00\00\00\01\00\02\00\03\00\00\00")
      (data (i32.const 0x118)
        "\30\01\00\00\02\00\00\00\03\77\77\77" "\32\01\00\00\00\00\00\00\00\77\77\77")
      (data (i32.const 0x130) "hiab\ff\fe\07\00"))
    (core instance $memory (instantiate $Memory))
    (core func $read (canon lower (func $read)))
    (core func $stream.new (canon stream.new $ST))
    (core func $write (canon stream.write $ST async (memory (core memory $memory "mem"))))
    (core func $write-latin1 (canon stream.write $ST async (memory (core memory $memory "mem"))
      string-encoding=latin1+utf16))
    (core module $M
      (import "" "mem" (memory 1))
      (import "" "read" (func $read (param i32) (result i32)))
      (import "" "stream.new" (func $stream.new (result i64)))
      (import "" "write" (func $write (param i32 i32 i32) (result i32)))
      (import "" "write-latin1" (func $write-latin1 (param i32 i32 i32) (result i32)))
      (func (export "run") (param $latin1 i32) (param $addr i32) (param $val i32) (result i32)
        (local $s i64) (local $tx i32)
        (if (local.get $addr) (then (i32.store (local.get $addr) (local.get $val))))
        (local.set $s (call $stream.new))
        (local.set $tx (i32.wrap_i64 (i64.shr_u (local.get $s) (i64.const 32))))
        (if (local.get $latin1)
          (then (drop (call $write-latin1 (local.get $tx) (i32.const 16) (i32.const 2))))
          (else (drop (call $write (local.get $tx) (i32.const 16) (i32.const 2)))))
        (call $read (i32.wrap_i64 (local.get $s)))))
    (core instance $m (instantiate $M (with "" (instance
      (export "mem" (memory $memory "mem"))
      (export "read" (func $read))
      (export "stream.new" (func $stream.new))
      (export "write" (func $write))
      (export "write-latin1" (func $write-latin1))))))
    (func (export "run") (param "latin1" u32) (param "addr" u32) (param "val" u32) (result u32)
      (canon lift (core func $m "run")))
  )

  (instance $c (instantiate $C (with "e" (type $e)) (with "f" (type $f)) (with "rec" (type $rec))))
  (instance $d (instantiate $D (with "e" (type $e)) (with "f" (type $f)) (with "rec" (type $rec))
    (with "read" (func $c "read"))))
  (func (export "run") (alias export $d "run"))
)

(component instance $T $T)
(assert_return (invoke "run" (u32.const 0) (u32.const 0) (u32.const 0)) (u32.const 0xdecb057b))
(component instance $T $T)
(assert_return (invoke "run" (u32.const 1) (u32.const 0) (u32.const 0)) (u32.const 0xdecb057b))

;; The second string isn't UTF-8 (which is fine for latin1).
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 0) (u32.const 64) (u32.const 0x134)) "invalid utf-8")

;; The second string, or the second string in the first list, is too long.
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 0) (u32.const 68) (u32.const 0x10000))
  "string pointer/length out of bounds of memory")
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 1) (u32.const 68) (u32.const 0x10000))
  "string pointer/length out of bounds of memory")
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 0) (u32.const 0x128) (u32.const 0x10000))
  "string pointer/length out of bounds of memory")
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 1) (u32.const 0x128) (u32.const 0x10000))
  "string pointer/length out of bounds of memory")

;; The second list is too long, or misaligned.
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 0) (u32.const 76) (u32.const 0x8000))
  "list pointer/length out of bounds of memory")
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 1) (u32.const 76) (u32.const 0x8000))
  "list pointer/length out of bounds of memory")
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 0) (u32.const 72) (u32.const 0x137)) "list pointer is not aligned")
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 1) (u32.const 72) (u32.const 0x137)) "list pointer is not aligned")

;; The second char isn't a Unicode scalar value.
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 0) (u32.const 84) (u32.const 0xd800)) "out of range for `char`")
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 1) (u32.const 84) (u32.const 0xd800)) "out of range for `char`")

;; The second option's, or enum's, discriminant is out of range.
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 0) (u32.const 88) (u32.const 2)) "discriminant 2 out of range [0..2)")
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 1) (u32.const 88) (u32.const 2)) "discriminant 2 out of range [0..2)")
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 0) (u32.const 100) (u32.const 3)) "discriminant 3 out of range [0..3)")
(component instance $T $T)
(assert_trap (invoke "run" (u32.const 1) (u32.const 100) (u32.const 3)) "discriminant 3 out of range [0..3)")
