;;! component_model_async = true

;; $C writes the same 8 bytes to two streams, which the outer component reads.
;; The record's numbers fill every byte, so it's copied as is (with `memcpy`),
;; but the tuple's second byte is padding, which the reader's buffer must keep.
(component
  (component $C
    (type $R (record (field "a" (tuple u8 s8)) (field "b" u16)))
    (export $R' "r" (type $R))
    (type $S1 (stream $R'))
    (type $S2 (stream (tuple u8 u16)))
    (core module $Mem (memory (export "m") 1) (data (i32.const 0) "\01\02\03\04\05\06\07\08"))
    (core instance $mem (instantiate $Mem))
    (core module $M
      (import "" "new" (func $new (result i64)))
      (import "" "write" (func $write (param i32 i32 i32) (result i32)))
      (func (export "get") (result i32) (local $s i64)
        (local.set $s (call $new))
        (drop (call $write (i32.wrap_i64 (i64.shr_u (local.get $s) (i64.const 32))) (i32.const 0) (i32.const 2)))
        (i32.wrap_i64 (local.get $s))))
    (core func $new1 (canon stream.new $S1))
    (core func $write1 (canon stream.write $S1 async (memory (core memory $mem "m"))))
    (core func $new2 (canon stream.new $S2))
    (core func $write2 (canon stream.write $S2 async (memory (core memory $mem "m"))))
    (core instance $m1 (instantiate $M (with "" (instance (export "new" (func $new1)) (export "write" (func $write1))))))
    (core instance $m2 (instantiate $M (with "" (instance (export "new" (func $new2)) (export "write" (func $write2))))))
    (func (export "get1") (result $S1) (canon lift (core func $m1 "get")))
    (func (export "get2") (result $S2) (canon lift (core func $m2 "get"))))

  (instance $c (instantiate $C))
  (alias export $c "r" (type $R))
  (type $S1 (stream $R))
  (type $S2 (stream (tuple u8 u16)))
  (core module $Mem (memory (export "m") 1))
  (core instance $mem (instantiate $Mem))
  (core module $M
    (import "" "m" (memory 1))
    (import "" "get1" (func $get1 (result i32)))
    (import "" "read1" (func $read1 (param i32 i32 i32) (result i32)))
    (import "" "get2" (func $get2 (result i32)))
    (import "" "read2" (func $read2 (param i32 i32 i32) (result i32)))
    (func (export "run")
      ;; Both reads complete at once with 2 items.
      (if (i32.ne (call $read1 (call $get1) (i32.const 0) (i32.const 2)) (i32.const 0x20)) (then unreachable))
      (if (i32.ne (call $read2 (call $get2) (i32.const 8) (i32.const 2)) (i32.const 0x20)) (then unreachable))
      (if (i64.ne (i64.load (i32.const 0)) (i64.const 0x0807060504030201)) (then unreachable))
      (if (i64.ne (i64.load (i32.const 8)) (i64.const 0x0807000504030001)) (then unreachable))))
  (core func $get1 (canon lower (func $c "get1")))
  (core func $read1 (canon stream.read $S1 async (memory (core memory $mem "m"))))
  (core func $get2 (canon lower (func $c "get2")))
  (core func $read2 (canon stream.read $S2 async (memory (core memory $mem "m"))))
  (core instance $m (instantiate $M (with "" (instance (export "m" (memory $mem "m"))
    (export "get1" (func $get1)) (export "read1" (func $read1))
    (export "get2" (func $get2)) (export "read2" (func $read2))))))
  (func (export "run") (canon lift (core func $m "run"))))

(assert_return (invoke "run"))
