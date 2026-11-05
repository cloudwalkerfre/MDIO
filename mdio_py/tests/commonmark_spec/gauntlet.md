#5 不是标题
####### 七个 # 也不是
\## 转义也不算
# 一级 #
### 三级 ###
# foo \#
Foo
---
Foo

---
* * *
- - -
Foo
= =
Foo
    ---
1\. <http://x> 14. <http://x> <foo@bar.com>
__a<https://foo.bar/?q=__>

foo
14. <https://x> 1234567890. \* -1. __a<https://x/?q=__>
- <http://x/y z> <> <m:abc> <foo.bar.baz> \* \q &copy; <foo@example.com>
foo
*
-		foo
foo     
bar

<script>
*foo*
</script>

> `code` *em* **strong** [foo`]` code先 \\*em* &copy; <foo@example.com> &#35;
lazy: <foo@example.com> &copy; &notit; &amp &#35; \* \q &#42;em&#42;
>
> > <a href="`"> <33> <__> [foo](/f&ouml;) &amp; &#35;
> > > ___foo___ ____foo____ ` 剥 ` `多
> > > 行` `` `内` `` ```foo`` a*"foo"* *foo _bar* baz_
> > > > 1. <http://x/y z> <m:abc> <foo.bar.baz> *em* `code` &copy; \*
> > > > 2. *em* `code` ***both*** &copy; [inline](/url '单') __a<https://x/?q=__> <foo@example.com> &#35; \q
> > > >    1) &notit; [foo](</my>) **foo **bar baz** &amp; <33> <__>
> > > >    2) __a<https://x/?q=__> [foo](foo(bar)) [inline]() <m:abc> &#42;em&#42;
> > > > 3. <http://x> [inline](foo\)b) \* _inside_ [foo](/f&ouml;) ***both*** ` 剥 `
> > > >    > [inside]: /url/inside
> > > >    > `*`字面 ![alt](/img) ![foo [bar](/x)](/u) [foo [bar](/x)](/u) <foo@example.com> &amp;
> > > >    > > [折叠][] [快捷] [ref][ID] [&copy;][&copy;] \* <https://x>
> > > >    > > - <foo@example.com> &amp; &#35; [inline]() ***both*** `code` *em* [foo](</my>)
> > > >    > >   - - `*`字面 &copy; <https://x> [foo](/f&ouml;) <m:abc> \q
> > > >    > >     ``` 
> > > >    > >     unclosed `code` *em* [link](url) &copy; &#35; \* &notit;
> > > >    > >     <https://x> <foo@example.com> <m:abc> <foo.bar.baz> ***both***

[全引用][id] [折叠][] [快捷] [ref][ID] [&copy;][&copy;] [inside] [foo`bar] [多行] [foo]: <bar>(baz)

[id]: /url/id "定义"
[ID]: /url/dup
[快捷]: /url/shortcut
[&copy;]: /url/copy
[多行]: /url
  'title'
