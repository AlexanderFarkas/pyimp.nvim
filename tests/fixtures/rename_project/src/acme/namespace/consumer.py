from acme.namespace.old import NamespaceThing
from acme.namespace import old
from .old import NAMESPACE_VALUE
from acme.namespace.subpkg.mod import SubpkgThing
from acme.namespace.subpkg import mod


def use_namespace_consumer():
    return NamespaceThing, old.NAMESPACE_VALUE, NAMESPACE_VALUE, SubpkgThing, mod.SUBPKG_VALUE
