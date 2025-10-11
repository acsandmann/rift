## values
rift has a few core values that it strives to uphold:
- performance:
performance is key. rift aims to be as performant as possible and sacrifices are acceptable in the name of performance. this means that rift will use private apis where necessary to achieve its performance goals.
- usability:
i want rift to be easy to use and configure. ideally this means that rift will eventually have a gui and/or cli to configure it(and for an easy/simple quick start). for now, rift can only be configured via a toml file but this is viewed as a serious shortcoming.
- stability:
once rift reaches a 1.0 release, i will try my hardest to avoid breaking changes and ensure that updating rift does **not** mean changing your config or losing features.

## non values
- use native macos spaces:
this is impossible without disabling sip which is not something that is on the table for rift.
- avoid using private apis:
in contrast to AeroSpace i believe that private/undocumented api's are far more stable and reliable than the public (accessibility) api. whilst rift has to make use of the ax api it tries to avoid doing so as that is blocking and detrimental to performance. also, the private api's used by rift are the basis of the accessibility api and are thus likely to not only be stable, but more performant, provided they are used correctly.
- avoid unsafe code:
rift will *try* to avoid unsafe code but the nature of an application that interfaces heavily with the operating system means that *somewhere* there will inevitably be a big, nasty, ball of unsafe code. rift will try to keep this ball as small as possible and isolate it from the rest of the codebase.
