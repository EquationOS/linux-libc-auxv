/*
MIT License

Copyright (c) 2025 Philipp Schuster

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/
use crate::util::get_null_index;
use crate::{AuxVar, AuxVarRaw, AuxVarType};
use aligned_vec::{ABox, AVec};
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::CStr;

/// Builder to create a stack layout as described by the [`StackLayoutRef`]
/// type.
///
/// [`StackLayoutRef`]: crate::StackLayoutRef
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StackLayoutBuilder<'a> {
    argv: Vec<String>,
    envv: Vec<String>,
    auxv: Vec<AuxVar<'a>>,
}

impl<'a> StackLayoutBuilder<'a> {
    /// Creates a mew bioöder-
    #[must_use]
    pub const fn new() -> Self {
        Self {
            argv: vec![],
            envv: vec![],
            auxv: vec![],
        }
    }

    /// Adds an argument to the builder.
    ///
    /// Adding a terminating NUL byte is not necessary. Interim NUL bytes are
    /// prohibited.
    pub fn add_argv(&mut self, arg: impl Into<String>) {
        let mut arg = arg.into();
        if let Some(pos) = arg.find('\0') {
            assert_eq!(
                pos,
                arg.len() - 1,
                "strings must not contain interim NUL bytes"
            );
        }

        if !arg.ends_with('\0') {
            arg.push('\0');
        }

        self.argv.push(arg);
    }

    /// Adds an environment-variable to the builder.
    ///
    /// Adding a terminating NUL byte is not necessary. Interim NUL bytes are
    /// prohibited.
    ///
    /// The value must follow the `key=value` syntax, where `value` may be
    /// empty.
    pub fn add_envv(&mut self, env: impl Into<String>) {
        let mut env = env.into();
        if let Some(pos) = env.find('\0') {
            assert_eq!(
                pos,
                env.len() - 1,
                "strings must not contain interim NUL bytes"
            );
        }

        if !env.ends_with('\0') {
            env.push('\0');
        }

        // Check syntax
        {
            let (key, _value) = env
                .split_once('=')
                .expect("should have ENV var syntax (`key=value`)");
            assert!(!key.is_empty());
        }
        self.envv.push(env);
    }

    /// Adds an [`AuxVar`] to the builder.
    pub fn add_auxv(&mut self, aux: AuxVar<'a>) {
        // Ignore, we do this automatically in the end.
        if aux != AuxVar::Null {
            self.auxv.push(aux);
        }
    }

    /// Returns the size in bytes needed for the `argv` entries.
    ///
    /// This includes the terminating null entry.
    fn calc_len_argv_entries(&self) -> usize {
        (self.argv.len() + 1/* null */) * size_of::<usize>()
    }

    /// Returns the size in bytes needed for the `envv` entries.
    ///
    /// This includes the terminating null entry.
    fn calc_len_envv_entries(&self) -> usize {
        (self.envv.len() + 1/* null */) * size_of::<usize>()
    }

    /// Returns the size in bytes needed for the `auxv` entries.
    ///
    /// This includes the terminating null entry.
    fn calc_len_auxv_entries(&self) -> usize {
        (self.auxv.len() + 1/* NULL entry */) * size_of::<AuxVarRaw>()
    }

    fn _calc_len_data_cstr(strs: &[String]) -> usize {
        strs.iter()
            .map(|arg| arg.as_bytes())
            .map(|bytes| CStr::from_bytes_until_nul(bytes).expect("should have NUL byte"))
            .map(|cstr| cstr.count_bytes() + 1 /* NUL */)
            .sum::<usize>()
    }

    /// Returns the size in bytes needed for the `argv` data area.
    ///
    /// This includes any terminating null entries or padding.
    fn calc_len_argv_data(&self) -> usize {
        Self::_calc_len_data_cstr(&self.argv)
    }

    /// Returns the size in bytes needed for the `envv` data area.
    ///
    /// This includes any terminating null entries or padding.
    fn calc_len_envv_data(&self) -> usize {
        Self::_calc_len_data_cstr(&self.envv)
    }

    /// Returns the size in bytes needed for the `auxv` data area.
    ///
    /// This includes any terminating null entries or padding.
    fn calc_len_auxv_data(&self) -> usize {
        self.auxv
            .iter()
            .map(|aux| {
                match aux {
                    AuxVar::Platform(v) => {
                        v.count_bytes() + 1 /* NUL */
                    }
                    AuxVar::BasePlatform(v) => {
                        v.count_bytes() + 1 /* NUL */
                    }
                    AuxVar::Random(v) => {
                        assert_eq!(v.len(), 16);
                        16 /* fixed size */
                    }
                    AuxVar::ExecFn(v) => {
                        v.count_bytes() + 1 /* NUL */
                    }
                    _ => 0,
                }
            })
            .sum::<usize>()
    }

    /// Returns the total size in bytes needed for the structure.
    ///
    /// This includes any null entries or padding.
    fn calc_total_len(&self) -> usize {
        size_of::<usize>() /* argc */ +
            self.calc_len_argv_entries()
            + self.calc_len_envv_entries()
            + self.calc_len_auxv_entries()
            + self.calc_len_argv_data()
            + self.calc_len_envv_data()
            + self.calc_len_auxv_data()
    }

    /// Builds the layout with heap-allocated memory.
    #[must_use]
    pub fn build(mut self) -> ABox<[u8]> {
        if Some(&AuxVar::Null) != self.auxv.last() {
            self.auxv.push(AuxVar::Null);
        }

        // Zeroed buffer. Enables us to not write dedicated NULL entries into
        // `argv` and `envv`.
        let mut buffer = {
            let len = self.calc_total_len();
            let mut vec = AVec::<u8>::new(align_of::<usize>());
            for _ in 0..len {
                vec.push(0);
            }
            vec.into_boxed_slice()
        };

        let mut serializer = StackLayoutSerializer::new(
            &mut buffer,
            self.calc_len_argv_entries(),
            self.calc_len_envv_entries(),
            self.calc_len_auxv_entries(),
            self.calc_len_argv_data(),
            self.calc_len_envv_data(),
            self.calc_len_auxv_data(),
        );
        serializer.write_argc(self.argv.len());

        for arg in self.argv {
            let c_str = CStr::from_bytes_until_nul(arg.as_bytes()).unwrap();
            serializer.write_arg(c_str);
        }
        // Writing NULL entry not necessary, the buffer is already zeroed

        for var in self.envv {
            let c_str = CStr::from_bytes_until_nul(var.as_bytes()).unwrap();
            serializer.write_env(c_str);
        }
        // Writing NULL entry not necessary, the buffer is already zeroed

        for var in self.auxv {
            serializer.write_aux(&var);
        }

        buffer
    }

    /// Builds the layout on pre-allocated stack memory.
    ///
    /// # Arguments
    /// - `stack_top`: The top of the stack where the layout should be built.
    ///
    /// # Returns
    /// A tuple containing the base address of the current stack frame and the
    /// total size in bytes of the stack layout.
    ///
    #[must_use]
    pub fn build_on_stack(mut self, stack_top: usize) -> (usize, usize) {
        if Some(&AuxVar::Null) != self.auxv.last() {
            self.auxv.push(AuxVar::Null);
        }

        let len = self.calc_total_len();

        let (mut buffer, stack_base) = {
            // If a target address is given, we allocate the buffer with
            // the given alignment.
            // x86_64 calling convention: the stack must be 16-byte aligned before
            // calling a function.
            let stack_base = (stack_top - len) & !(align_of::<usize>() * 2 - 1);
            let stack_range = unsafe {
                // Zeroed the buffer.
                core::ptr::write_bytes(stack_base as *mut u8, 0, len);
                core::slice::from_raw_parts_mut(stack_base as *mut u8, len)
            };
            (stack_range, stack_base)
        };

        let mut serializer = StackLayoutSerializer::new(
            &mut buffer,
            self.calc_len_argv_entries(),
            self.calc_len_envv_entries(),
            self.calc_len_auxv_entries(),
            self.calc_len_argv_data(),
            self.calc_len_envv_data(),
            self.calc_len_auxv_data(),
        );

        serializer.write_argc(self.argv.len());

        for arg in self.argv {
            let c_str = CStr::from_bytes_until_nul(arg.as_bytes()).unwrap();
            serializer.write_arg(c_str);
        }
        // Writing NULL entry not necessary, the buffer is already zeroed

        for var in self.envv {
            let c_str = CStr::from_bytes_until_nul(var.as_bytes()).unwrap();
            serializer.write_env(c_str);
        }
        // Writing NULL entry not necessary, the buffer is already zeroed

        for var in self.auxv {
            serializer.write_aux(&var);
        }

        (stack_base, len)
    }
}

/// Serializer for [`StackLayoutBuilder`].
///
/// This type takes care of the _entry area_ and the _data area_ with respect
/// to a given `target_addr` (base address in target address space).
///
/// All strings can contain a NUL byte already. If it is not present, the
/// serializer will take care of that.
struct StackLayoutSerializer<'a> {
    buffer: &'a mut [u8],
    // Offset in bytes for writes
    offset_argv: usize,
    // Offset in bytes for writes
    offset_envv: usize,
    // Offset in bytes for writes
    offset_auxv: usize,
    // Offset in bytes for writes
    offset_argv_data: usize,
    // Offset in bytes for writes
    offset_envv_data: usize,
    // Offset in bytes for writes
    offset_auxv_data: usize,
}

impl<'a> StackLayoutSerializer<'a> {
    /// Creates a new builder.
    ///
    /// The `auxv` entries [`AuxVarType::Null`] will be added automatically.
    ///
    /// # Arguments
    /// - `target_addr`: The address the stack layout in the target address space.
    ///   This may be a user-space address of another process.
    #[allow(clippy::too_many_arguments)]
    fn new(
        buffer: &'a mut [u8],
        len_argv_entries: usize,
        len_envv_entries: usize,
        len_auxv_entries: usize,
        len_argv_data: usize,
        len_envv_data: usize,
        len_auxv_data: usize,
    ) -> Self {
        assert_eq!(buffer.as_ptr().align_offset(align_of::<usize>()), 0);

        let total_size = size_of::<usize>() /* initial argc */ + len_argv_entries + len_envv_entries + len_auxv_entries
            + len_argv_data + len_envv_data + len_auxv_data;
        assert!(buffer.len() >= total_size);

        // These offsets include any necessary NULL entries and NUL bytes.
        let offset_argv = size_of::<usize>() /* initial argc */;
        let offset_envv = offset_argv + len_argv_entries;
        let offset_auxv = offset_envv + len_envv_entries;
        // auxv data area comes first, then argv, then envv
        let offset_auxv_data = offset_auxv + len_auxv_entries;
        let offset_argv_data = offset_auxv_data + len_auxv_data;
        let offset_envv_data = offset_argv_data + len_argv_data;

        Self {
            buffer,
            offset_argv: size_of::<usize>(), /* argc */
            offset_envv,
            offset_auxv,
            offset_argv_data,
            offset_envv_data,
            offset_auxv_data,
        }
    }

    /// Performs sanity checks ensuring that no offset breaks its boundaries.
    fn sanity_checks(&self) {
        assert!(self.offset_argv <= self.offset_envv);
        assert!(self.offset_envv <= self.offset_auxv);
        assert!(self.offset_auxv <= self.offset_auxv_data);
        assert!(self.offset_auxv_data <= self.offset_argv_data);
        assert!(self.offset_argv_data <= self.offset_envv_data);
        assert!(self.offset_envv_data <= self.buffer.len());
    }

    /// Writes bytes to the data area and updates the offset afterward.
    const fn _write_data_area(buffer: &mut [u8], data: &[u8], data_area_offset: &mut usize) {
        let src_ptr = data.as_ptr();
        let dst_ptr = buffer.as_mut_ptr().cast::<u8>();
        let dst_ptr = unsafe { dst_ptr.add(*data_area_offset) };
        unsafe {
            core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, data.len());
        }
        *data_area_offset += data.len();
    }

    /// Writes a null-terminated CStr into the structure, including the
    /// pointer and the actual data.
    fn _write_cstr(
        buffer: &mut [u8],
        str: &CStr,
        entry_offset: &mut usize,
        data_area_offset: &mut usize,
    ) {
        // The address where this will be reachable from a user-perspective.
        let data_addr = buffer.as_ptr() as *const _ as usize + *data_area_offset;

        // write entry
        {
            let src_ptr = buffer.as_mut_ptr().cast::<u8>();
            let src_ptr = unsafe { src_ptr.add(*entry_offset) };
            unsafe { core::ptr::write(src_ptr.cast::<usize>(), data_addr) }
            *entry_offset += size_of::<usize>();
        }

        // write data
        Self::_write_data_area(buffer, str.to_bytes(), data_area_offset);
        // write NUL
        Self::_write_data_area(buffer, &[0], data_area_offset);
    }

    /// Writes the `argc` value into the structure.
    fn write_argc(&mut self, argc: usize) {
        unsafe { core::ptr::write(self.buffer.as_mut_ptr().cast::<usize>(), argc) }

        self.sanity_checks();
    }

    /// Writes an argument into the structure.
    fn write_arg(&mut self, arg: &CStr) {
        Self::_write_cstr(
            self.buffer,
            arg,
            &mut self.offset_argv,
            &mut self.offset_argv_data,
        );
        self.sanity_checks();
    }

    /// Writes an environmental variable into the structure.
    fn write_env(&mut self, var: &CStr) {
        Self::_write_cstr(
            self.buffer,
            var,
            &mut self.offset_envv,
            &mut self.offset_envv_data,
        );

        self.sanity_checks();
    }

    /// Writes an auxiliary variable into the auxiliary vector.
    fn write_aux_immediate(&mut self, key: AuxVarType, val: usize) {
        let ptr = self.buffer.as_mut_ptr().cast::<u8>();
        let ptr = unsafe { ptr.add(self.offset_auxv) };
        let value = AuxVarRaw::new(key, val);
        unsafe { core::ptr::write(ptr.cast::<AuxVarRaw>(), value) }
        self.offset_auxv += size_of::<AuxVarRaw>();
    }

    /// Writes the referenced data of an auxiliary vector into the
    /// _auxv data area_.
    fn write_aux_refdata(&mut self, key: AuxVarType, data: &[u8], add_nul_byte: bool) {
        // The address where this will be reachable from a user-perspective.
        let data_addr = self.buffer.as_ptr() as *const _ as usize + self.offset_auxv_data;
        self.write_aux_immediate(key, data_addr);

        // write data
        Self::_write_data_area(self.buffer, data, &mut self.offset_auxv_data);

        // add NUL byte if necessary
        if add_nul_byte {
            // assert: If there is a NUL byte in the data, it is only allowed at
            // the very last position
            if let Some(pos) = get_null_index(data) {
                assert_eq!(
                    pos,
                    data.len() - 1,
                    "strings must not contain interim NUL bytes"
                );
            }

            if data.last().copied().unwrap() != 0 {
                // write NUL
                Self::_write_data_area(self.buffer, &[0], &mut self.offset_auxv_data);
            }
        }
    }

    /// Deconstructs a [`AuxVar`] and writes the corresponding [`AuxVarRaw`]
    /// into the structure.
    fn write_aux(&mut self, aux: &AuxVar<'a>) {
        match aux {
            AuxVar::Platform(v) => self.write_aux_refdata(aux.key(), v.as_bytes(), true),
            AuxVar::BasePlatform(v) => self.write_aux_refdata(aux.key(), v.as_bytes(), true),
            AuxVar::Random(v) => self.write_aux_refdata(aux.key(), v, false),
            AuxVar::ExecFn(v) => self.write_aux_refdata(aux.key(), v.as_bytes(), true),
            _ => self.write_aux_immediate(aux.key(), aux.value_raw()),
        }

        self.sanity_checks();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StackLayoutRef;

    #[test]
    fn test_builder() {
        let mut builder = StackLayoutBuilder::new();
        builder.add_argv("first arg");
        builder.add_argv("second arg");
        builder.add_envv("var1=foo");
        builder.add_envv("var2=bar");
        builder.add_auxv(AuxVar::EGid(1_1337));
        builder.add_auxv(AuxVar::Gid(2_1337));
        builder.add_auxv(AuxVar::Random([
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        ]));
        builder.add_auxv(AuxVar::Uid(3_1337));
        builder.add_auxv(AuxVar::ExecFn(c"ExecFn as &CStr".into()));
        builder.add_auxv(AuxVar::Platform("Platform as &str".into()));
        builder.add_auxv(AuxVar::BasePlatform("Base Platform as &str".into()));
        let layout = builder.build();

        // now parse the layout
        let layout = StackLayoutRef::new(layout.as_ref(), None);

        assert_eq!(layout.argc(), 2);

        // argv
        {
            assert_eq!(layout.argv_raw_iter().count(), 2);

            // Just printing uncovers memory errors
            // SAFETY: This was created for the address space of this process.
            unsafe { layout.argv_iter() }
                .enumerate()
                .for_each(|(i, str)| eprintln!("  arg {i:>2}: {str:?}"));
        }

        // envv
        {
            assert_eq!(layout.envc(), 2);

            // Just printing uncovers memory errors
            // SAFETY: This was created for the address space of this process.
            unsafe { layout.envv_iter() }
                .enumerate()
                .for_each(|(i, ptr)| eprintln!("  env {i:>2}: {ptr:?}"));
        }

        // auxv
        {
            assert_eq!(layout.auxvc(), 7);

            // Just printing uncovers memory errors
            // SAFETY: This was created for the address space of this process.
            unsafe { layout.auxv_iter() }
                .enumerate()
                .for_each(|(i, aux)| eprintln!("  auxv {i:>2}: {aux:?}"));
        }

        // now test the strings match
        let fn_get_at_string = |key: AuxVarType| {
            unsafe { layout.auxv_iter() }
                .find(|e| e.key() == key)
                .map(|v| {
                    v.value_payload_str()
                        .cloned()
                        .map(|s| s.into_string())
                        .unwrap()
                })
                .unwrap()
        };
        let at_exec_fn = fn_get_at_string(AuxVarType::ExecFn);
        assert_eq!(at_exec_fn, "ExecFn as &CStr");

        let at_base_platform = fn_get_at_string(AuxVarType::BasePlatform);
        assert_eq!(at_base_platform, "Base Platform as &str");
    }
}
