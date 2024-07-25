use anyhow::Result;

use cap_std::fs::{Dir, File, Permissions, PermissionsExt};
use cap_std_ext::cmdext::CapStdExtCommandExt;
use cap_std_ext::dirext::CapStdExtDirExt;
use cap_std_ext::{cap_std, RootDir};
use std::io::Write;
use std::path::Path;
use std::{process::Command, sync::Arc};

#[test]
fn take_fd() -> Result<()> {
    let mut c = Command::new("/bin/bash");
    c.arg("-c");
    c.arg("wc -c <&5");
    let (r, w) = rustix::pipe::pipe()?;
    let r = Arc::new(r);
    let mut w: File = w.into();
    c.take_fd_n(r, 5);
    write!(w, "hello world")?;
    drop(w);
    c.stdout(std::process::Stdio::piped());
    let s = c.output()?;
    assert!(s.status.success());
    assert_eq!(s.stdout.as_slice(), b"11\n");
    Ok(())
}

#[test]
fn fchdir() -> Result<()> {
    static CONTENTS: &[u8] = b"hello world";

    fn new_cmd() -> Command {
        let mut c = Command::new("/usr/bin/cat");
        c.arg("somefile");
        c
    }

    fn test_cmd(mut c: Command) -> Result<()> {
        let st = c.output()?;
        if !st.status.success() {
            anyhow::bail!("Failed to exec cat");
        }
        assert_eq!(st.stdout.as_slice(), CONTENTS);
        Ok(())
    }

    let td = Arc::new(cap_tempfile::tempdir(cap_std::ambient_authority())?);

    td.write("somefile", CONTENTS)?;

    let mut c = new_cmd();
    let subdir = td.open_dir(".")?;
    c.cwd_dir(subdir.try_clone()?);
    test_cmd(c).unwrap();

    Ok(())
}

#[test]
fn optionals() -> Result<()> {
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;

    // file
    assert!(td.open_optional("bar")?.is_none());
    assert!(td.metadata_optional("bar").unwrap().is_none());
    assert!(!(td.remove_file_optional("bar")?));
    td.write("bar", "testcontents")?;
    assert!(td.metadata_optional("bar").unwrap().is_some());
    assert!(td.symlink_metadata_optional("bar").unwrap().is_some());
    assert_eq!(td.read("bar")?.as_slice(), b"testcontents");
    assert!(td.remove_file_optional("bar")?);

    // directory
    assert!(td.open_dir_optional("somedir")?.is_none());
    td.create_dir("somedir")?;
    assert!(td.open_dir_optional("somedir")?.is_some());
    Ok(())
}

#[test]
fn ensuredir() -> Result<()> {
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;

    let p = Path::new("somedir");
    let b = &cap_std::fs::DirBuilder::new();
    assert!(td.metadata_optional(p)?.is_none());
    assert!(td.symlink_metadata_optional(p)?.is_none());
    assert!(td.ensure_dir_with(p, b).unwrap());
    assert!(td.metadata_optional(p)?.is_some());
    assert!(td.symlink_metadata_optional(p)?.is_some());
    assert!(!td.ensure_dir_with(p, b).unwrap());

    let p = Path::new("somedir/without/existing-parent");
    // We should fail because the intermediate directory doesn't exist.
    assert!(td.ensure_dir_with(p, b).is_err());
    // Now create the parent
    assert!(td.ensure_dir_with(p.parent().unwrap(), b).unwrap());
    assert!(td.ensure_dir_with(p, b).unwrap());
    assert!(!td.ensure_dir_with(p, b).unwrap());

    // Verify we don't replace a file
    let p = Path::new("somefile");
    td.write(p, "some file contents")?;
    assert!(td.ensure_dir_with(p, b).is_err());

    // Broken symlinks aren't followed and are errors
    let p = Path::new("linksrc");
    td.symlink("linkdest", p)?;
    assert!(td.metadata(p).is_err());
    assert!(td
        .symlink_metadata_optional(p)
        .unwrap()
        .unwrap()
        .is_symlink());
    // Non-broken symlinks are also an error
    assert!(td.ensure_dir_with(p, b).is_err());
    td.create_dir("linkdest")?;
    assert!(td.ensure_dir_with(p, b).is_err());
    assert!(td.metadata_optional(p).unwrap().unwrap().is_dir());

    Ok(())
}

#[test]
fn test_remove_all_optional() -> Result<()> {
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;

    let p = Path::new("somedir");
    assert!(!td.remove_all_optional(p).unwrap());
    td.create_dir(p)?;
    assert!(td.remove_all_optional(p).unwrap());
    let subpath = p.join("foo/bar");
    td.create_dir_all(subpath)?;
    assert!(td.remove_all_optional(p).unwrap());

    // regular file
    td.write(p, "test")?;
    assert!(td.remove_all_optional(p).unwrap());

    // symlinks; broken and not
    let p = Path::new("linksrc");
    td.symlink("linkdest", p)?;
    assert!(td.remove_all_optional(p).unwrap());
    td.symlink("linkdest", p)?;
    assert!(td.remove_all_optional(p).unwrap());

    Ok(())
}

/// Hack to determine the default mode for a file; we could
/// on Linux actually parse /proc/self/umask as is done in cap_tempfile,
/// but eh this is just to cross check with that code.
fn default_mode(d: &Dir) -> Result<Permissions> {
    let f = cap_tempfile::TempFile::new(d)?;
    Ok(f.as_file().metadata()?.permissions())
}

#[test]
fn link_tempfile_with() -> Result<()> {
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;
    let p = Path::new("foo");
    td.atomic_replace_with(p, |f| writeln!(f, "hello world"))
        .unwrap();
    assert_eq!(td.read_to_string(p).unwrap().as_str(), "hello world\n");
    let default_perms = default_mode(&td)?;
    assert_eq!(td.metadata(p)?.permissions(), default_perms);

    td.atomic_replace_with(p, |f| writeln!(f, "atomic replacement"))
        .unwrap();
    assert_eq!(
        td.read_to_string(p).unwrap().as_str(),
        "atomic replacement\n"
    );

    let e = td
        .atomic_replace_with(p, |f| {
            writeln!(f, "should not be written")?;
            Err::<(), _>(std::io::Error::new(std::io::ErrorKind::Other, "oops"))
        })
        .err()
        .unwrap();
    assert!(e.to_string().contains("oops"));
    // We should not have written to the file!
    assert_eq!(
        td.read_to_string(p).unwrap().as_str(),
        "atomic replacement\n"
    );

    td.atomic_write(p, "atomic replacement write\n").unwrap();
    assert_eq!(
        td.read_to_string(p).unwrap().as_str(),
        "atomic replacement write\n"
    );
    assert_eq!(td.metadata(p)?.permissions(), default_perms);

    td.atomic_write_with_perms(p, "atomic replacement 3\n", Permissions::from_mode(0o700))
        .unwrap();
    assert_eq!(
        td.read_to_string(p).unwrap().as_str(),
        "atomic replacement 3\n"
    );
    assert_eq!(td.metadata(p)?.permissions().mode() & 0o777, 0o700);

    // Ensure we preserve the executable bit on an existing file
    assert_eq!(td.metadata(p).unwrap().permissions().mode() & 0o700, 0o700);
    td.atomic_write(p, "atomic replacement 4\n").unwrap();
    assert_eq!(
        td.read_to_string(p).unwrap().as_str(),
        "atomic replacement 4\n"
    );
    assert_eq!(td.metadata(p)?.permissions().mode() & 0o777, 0o700);

    // But we should ignore permissions on a symlink (both existing and broken)
    td.remove_file(p)?;
    let p2 = Path::new("bar");
    td.atomic_write_with_perms(p2, "link target", Permissions::from_mode(0o755))
        .unwrap();
    td.symlink(p2, p)?;
    td.atomic_write(p, "atomic replacement symlink\n").unwrap();
    assert_eq!(td.metadata(p)?.permissions(), default_perms);
    // And break the link
    td.remove_file(p2)?;
    td.atomic_write(p, "atomic replacement symlink\n").unwrap();
    assert_eq!(td.metadata(p)?.permissions(), default_perms);

    // Also test with mode 0600
    td.atomic_write_with_perms(p, "self-only file", Permissions::from_mode(0o600))
        .unwrap();
    assert_eq!(td.metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
    td.atomic_write(p, "self-only file v2").unwrap();
    assert_eq!(td.metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
    // But we can override
    td.atomic_write_with_perms(p, "self-only file v3", Permissions::from_mode(0o640))
        .unwrap();
    assert_eq!(td.metadata(p).unwrap().permissions().mode() & 0o777, 0o640);
    Ok(())
}

#[test]
fn test_timestamps() -> Result<()> {
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;
    let p = Path::new("foo");
    td.atomic_replace_with(p, |f| writeln!(f, "hello world"))
        .unwrap();
    let ts0 = td.metadata(p)?.modified()?;
    // This test assumes at least second granularity on filesystem timestamps, and
    // that the system clock is not rolled back during the test.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let ts1 = td.metadata(p)?.modified()?;
    assert_eq!(ts0, ts1);
    td.update_timestamps(p).unwrap();
    let ts2 = td.metadata(p)?.modified()?;
    assert_ne!(ts1, ts2);
    assert!(ts2 > ts1);

    Ok(())
}

// For now just this test is copy-pasted to verify utf8
#[test]
#[cfg(feature = "fs_utf8")]
fn ensuredir_utf8() -> Result<()> {
    use cap_std::fs_utf8::camino::Utf8Path;
    use cap_std_ext::dirext::CapStdExtDirExtUtf8;
    let td = cap_tempfile::tempdir(cap_std::ambient_authority())?;
    let td = &cap_std::fs_utf8::Dir::from_cap_std((&*td).try_clone()?);

    let p = Utf8Path::new("somedir");
    let b = &cap_std::fs::DirBuilder::new();
    assert!(td.metadata_optional(p)?.is_none());
    assert!(td.symlink_metadata_optional(p)?.is_none());
    assert!(td.ensure_dir_with(p, b).unwrap());
    assert!(td.metadata_optional(p)?.is_some());
    assert!(td.symlink_metadata_optional(p)?.is_some());
    assert!(!td.ensure_dir_with(p, b).unwrap());

    let p = Utf8Path::new("somedir/without/existing-parent");
    // We should fail because the intermediate directory doesn't exist.
    assert!(td.ensure_dir_with(p, b).is_err());
    // Now create the parent
    assert!(td.ensure_dir_with(p.parent().unwrap(), b).unwrap());
    assert!(td.ensure_dir_with(p, b).unwrap());
    assert!(!td.ensure_dir_with(p, b).unwrap());

    // Verify we don't replace a file
    let p = Utf8Path::new("somefile");
    td.write(p, "some file contents")?;
    assert!(td.ensure_dir_with(p, b).is_err());

    // Broken symlinks aren't followed and are errors
    let p = Utf8Path::new("linksrc");
    td.symlink("linkdest", p)?;
    assert!(td.metadata(p).is_err());
    assert!(td
        .symlink_metadata_optional(p)
        .unwrap()
        .unwrap()
        .is_symlink());
    // Non-broken symlinks are also an error
    assert!(td.ensure_dir_with(p, b).is_err());
    td.create_dir("linkdest")?;
    assert!(td.ensure_dir_with(p, b).is_err());
    assert!(td.metadata_optional(p).unwrap().unwrap().is_dir());

    Ok(())
}

#[test]
#[cfg(feature = "fs_utf8")]
fn filenames_utf8() -> Result<()> {
    use std::collections::BTreeSet;

    use cap_std_ext::dirext::CapStdExtDirExtUtf8;
    let td = &cap_tempfile::utf8::TempDir::new(cap_std::ambient_authority())?;

    let mut expected = BTreeSet::new();
    const N: usize = 20;
    (0..N).try_for_each(|_| -> Result<()> {
        let fname = uuid::Uuid::new_v4().to_string();

        td.write(&fname, &fname)?;
        expected.insert(fname);
        Ok(())
    })?;
    let names = td.filenames_sorted().unwrap();
    for (a, b) in expected.iter().zip(names.iter()) {
        assert_eq!(a, b);
    }

    td.create_dir(".foo").unwrap();

    let names = td
        .filenames_filtered_sorted(|_ent, name| !name.starts_with('.'))
        .unwrap();
    assert_eq!(names.len(), N);
    for name in names.iter() {
        assert!(!name.starts_with('.'));
    }
    Ok(())
}

#[test]
fn test_rootdir_open() -> Result<()> {
    let td = &cap_tempfile::TempDir::new(cap_std::ambient_authority())?;
    let root = RootDir::new(td, ".").unwrap();

    assert!(root.open_optional("foo").unwrap().is_none());

    td.create_dir("etc")?;
    td.create_dir_all("usr/lib")?;

    let authjson = "usr/lib/auth.json";
    assert!(root.open(authjson).is_err());
    assert!(root.open_optional(authjson).unwrap().is_none());
    td.write(authjson, "auth contents")?;
    assert!(root.open_optional(authjson).unwrap().is_some());
    let contents = root.read_to_string(authjson).unwrap();
    assert_eq!(&contents, "auth contents");

    td.symlink_contents("/usr/lib/auth.json", "etc/auth.json")?;

    let contents = root.read_to_string("/etc/auth.json").unwrap();
    assert_eq!(&contents, "auth contents");

    // But this should fail due to an escape
    assert!(td.read_to_string("etc/auth.json").is_err());
    Ok(())
}

#[test]
fn test_rootdir_entries() -> Result<()> {
    let td = &cap_tempfile::TempDir::new(cap_std::ambient_authority())?;
    let root = RootDir::new(td, ".").unwrap();

    td.create_dir("etc")?;
    td.create_dir_all("usr/lib")?;

    let ents = root
        .entries()
        .unwrap()
        .collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(ents.len(), 2);
    Ok(())
}

#[test]
fn test_mountpoint() -> Result<()> {
    let root = &Dir::open_ambient_dir("/", cap_std::ambient_authority())?;
    assert_eq!(root.is_mountpoint(".").unwrap(), Some(true));
    let td = &cap_tempfile::TempDir::new(cap_std::ambient_authority())?;
    assert_eq!(td.is_mountpoint(".").unwrap(), Some(false));
    Ok(())
}
